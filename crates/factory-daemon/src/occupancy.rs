//! The occupancy chart: what every agent was doing over a window.
//!
//! Assembled here rather than in a UI, so the web page, the CLI and anything
//! that comes later draw the same picture from the same rules. Three layers,
//! deliberately kept apart:
//!
//! * **blocks** -- runs. Factory started them and the agent reported on them.
//!   Record, not inference.
//! * **planned** -- what a schedule says will happen next, drawn ahead of now.
//! * **spans** -- what the runtime saw. Weaker evidence, and the only layer
//!   that does not exist before Factory started writing it down.

use chrono::{DateTime, Duration, Utc};
use factory_core::adapter::{RuntimeStatus, StatusReport, StatusSource};
use factory_core::error::Result;
use factory_core::event::Event;
use factory_core::occupancy::{
    spans_from, Occupancy, OccupancyBlock, OccupancyPlan, OccupancyRow, OccupancyScope, StatusChange,
};
use factory_core::run::{BlockSource, Run, RunPatch, RunStatus};
use factory_core::task::TaskEntry;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::engine::Engine;

/// How far back the chart looks when nobody says.
const DEFAULT_MINUTES: u32 = 12 * 60;
/// A window wider than this is a different tool than a chart of today.
const MAX_MINUTES: u32 = 30 * 24 * 60;

impl Engine {
    pub async fn occupancy(self: &Arc<Self>, minutes: Option<u32>) -> Result<Occupancy> {
        let now = Utc::now();
        let minutes = minutes.unwrap_or(DEFAULT_MINUTES).clamp(5, MAX_MINUTES);
        let from = now - Duration::minutes(minutes as i64);
        // A quarter of the window is kept ahead of now. Without it the next
        // scheduled run is drawn on the right-hand edge, where it is a pixel
        // rather than a plan.
        let to = now + Duration::minutes((minutes / 4).max(1) as i64);

        let runs = self.store.runs_between(from, now).await?;
        let tasks = self.store.list(&Default::default()).await?;
        let titles: BTreeMap<&str, &str> = tasks
            .iter()
            .map(|t| (t.id.as_str(), t.title.as_str()))
            .collect();
        let estimates: BTreeMap<&str, u64> = tasks
            .iter()
            .filter_map(|t| t.estimate_seconds.map(|e| (t.id.as_str(), e)))
            .collect();

        // Runs by (scope, agent name). A run records the agent it was given to,
        // and the task records the scope -- key both the way the agents page
        // keys them, by name and never by harness. Canonicalized: a task
        // written before a scope's identity became its path still carries the
        // bare name it was given, and without this a run of one would land in
        // an orphan row of its own instead of on the scope's actual line.
        let scope_of: BTreeMap<&str, String> = tasks
            .iter()
            .map(|t| (t.id.as_str(), self.factory.canonical_scope_name(&t.scope)))
            .collect();
        let mut blocks: BTreeMap<(String, String), Vec<OccupancyBlock>> = BTreeMap::new();
        for run in &runs {
            let scope = scope_of.get(run.task_id.as_str()).cloned().unwrap_or_default();
            blocks
                .entry((scope, run.agent.clone()))
                .or_default()
                .push(block_of(
                    run,
                    titles.get(run.task_id.as_str()).copied(),
                    estimates.get(run.task_id.as_str()).copied(),
                ));
        }

        // What a schedule says is coming, drawn as wide as the task's own
        // history says it usually takes.
        let mut planned: BTreeMap<(String, String), Vec<OccupancyPlan>> = BTreeMap::new();
        for task in &tasks {
            let Some(at) = task.next_run_at else { continue };
            if at < now || at > to {
                continue;
            }
            let historical = if task.estimate_seconds.is_some() {
                (None, 0)
            } else {
                self.historical_estimate_for(&task.id).await
            };
            let (estimate, samples, user_estimate) =
                plan_estimate(task.estimate_seconds, historical);
            let scope = scope_of.get(task.id.as_str()).cloned().unwrap_or_default();
            planned
                .entry((scope, task.agent.clone()))
                .or_default()
                .push(OccupancyPlan {
                    task_id: task.id.clone(),
                    title: task.title.clone(),
                    at,
                    estimate_seconds: estimate,
                    samples,
                    user_estimate,
                });
        }

        // Liveness, grouped by the session it was observed on -- by subject,
        // never by agent. Two runs of the same agent at the same time are two
        // sessions with two independent states; interleaving them into one
        // series produces a strip that describes neither.
        //
        // Changes from before the window still matter: they say what state the
        // window opened in, so keep the last one before `from` as the opening
        // span.
        let changes = self.store.status_changes(from - Duration::hours(24)).await?;
        let mut spans: BTreeMap<String, Vec<StatusChange>> = BTreeMap::new();
        for change in changes {
            spans.entry(change.subject.clone()).or_default().push(change);
        }
        for series in spans.values_mut() {
            trim_to_window(series, from);
        }

        let liveness_since = self.store.status_origin().await?;

        let mut out = Vec::new();
        let (views, _available) = self.scope_views().await?;
        for view in views {
            let path = view.path.clone();
            let mut rows = Vec::new();
            for agent in &view.agents {
                let key = (view.name.clone(), agent.name.clone());
                let blocks = blocks.remove(&key).unwrap_or_default();
                let planned = planned.remove(&key).unwrap_or_default();
                // A row draws the standing agent's own session and nothing
                // else. A run's liveness is recorded too, but the run already
                // has a block on this row saying more than a screen can.
                let spans = spans
                    .remove(&format!("{}/{}", view.name, agent.name))
                    .map(|series| spans_from(&series, now))
                    .unwrap_or_default();
                let busy_seconds = busy_seconds(&blocks, from, now);
                rows.push(OccupancyRow {
                    agent: agent.name.clone(),
                    adapter: agent.adapter.clone(),
                    lifetime: agent.lifetime.clone(),
                    role: agent.role.clone(),
                    state: agent.state.clone(),
                    blocks,
                    planned,
                    spans,
                    busy_seconds,
                });
            }
            out.push(OccupancyScope {
                name: view.name,
                path,
                rows,
            });
        }

        // A run whose agent the config no longer declares still happened. Give
        // it a row rather than dropping it: a chart that hides work because
        // somebody edited a config is worse than one with an extra line.
        for ((scope, agent), blocks) in blocks {
            let busy_seconds = busy_seconds(&blocks, from, now);
            let row = OccupancyRow {
                agent,
                adapter: String::new(),
                lifetime: "task".into(),
                role: "worker".into(),
                state: "undeclared".into(),
                blocks,
                planned: Vec::new(),
                spans: Vec::new(),
                busy_seconds,
            };
            match out.iter_mut().find(|s| s.name == scope) {
                Some(existing) => existing.rows.push(row),
                None => out.push(OccupancyScope {
                    name: scope,
                    path: String::new(),
                    rows: vec![row],
                }),
            }
        }

        Ok(Occupancy {
            from,
            to,
            now,
            liveness_since,
            scopes: out,
        })
    }

    /// Write down that a session's liveness changed -- and only then.
    ///
    /// This is the whole of Factory's liveness history. The runtime has none:
    /// herdr will say what an agent is doing now and has no idea what it was
    /// doing an hour ago, so a status that is not appended here when it is
    /// seen is gone for good. What that costs is honest to state: the runtime
    /// is polled, so a flip and a flip back between two ticks leaves no trace.
    ///
    /// Skipping a sample when nothing changed is right for this record and
    /// wrong for anything that wants to know an agent is still doing what it
    /// was doing -- `agents.rs`'s pushed-event handler feeds this for the
    /// chart and publishes on the bus separately, on purpose, rather than
    /// folding that into here.
    pub(crate) async fn record_status(
        &self,
        subject: &str,
        scope: &str,
        agent: &str,
        status: RuntimeStatus,
    ) {
        {
            let mut seen = match self.seen_status.lock() {
                Ok(seen) => seen,
                // A poisoned lock is not a reason to take the daemon down over
                // a chart. Skip the sample and carry on.
                Err(_) => return,
            };
            if seen.get(subject) == Some(&status) {
                return;
            }
            seen.insert(subject.to_string(), status);
        }
        let change = StatusChange {
            subject: subject.to_string(),
            scope: scope.to_string(),
            agent: agent.to_string(),
            status,
            at: Utc::now(),
        };
        if let Err(e) = self.store.append_status(&change).await {
            tracing::debug!(subject, "could not record liveness: {e}");
        }
    }

    /// A session that is not there any more. Closes the open span rather than
    /// letting the chart draw it forward to now.
    pub(crate) async fn record_gone(&self, subject: &str, scope: &str, agent: &str) {
        self.record_status(subject, scope, agent, RuntimeStatus::Gone)
            .await;
    }

    /// Close every open liveness span on the way out. Nothing observes an
    /// agent while the daemon is down, and a span left open would be drawn
    /// straight through the outage as though someone had been watching.
    pub async fn close_liveness(self: &Arc<Self>) {
        for agent in self.store.agents().await.unwrap_or_default() {
            if agent.session.is_some() {
                self.record_gone(&agent.id, &agent.scope, &agent.name).await;
            }
        }
        for run in self.store.active_runs().await.unwrap_or_default() {
            let Ok(Some(task)) = self.store.get(&run.task_id).await else {
                continue;
            };
            self.record_gone(&format!("run:{}", run.id), &task.scope, &run.agent)
                .await;
        }
    }

    /// Poll every running run's session and write down what it says. The run
    /// itself is already a block on the chart; this is what the agent looked
    /// like while it held the bay.
    ///
    /// This is also the one place a hook-reported `blocked` (or the runtime
    /// saying working again) is acted on. The scheduler has its own poll of
    /// the runtime for the `Gone` check, but that is an existing, unrelated
    /// call to plain `status` -- putting the block/unblock logic here instead
    /// means `status_report`, the one call that can cost an extra
    /// subprocess (`herdr agent explain`), is asked for exactly once per run
    /// per tick, not twice.
    pub async fn record_run_liveness(self: &Arc<Self>) {
        let runs = self.store.active_runs().await.unwrap_or_default();
        for run in runs {
            if run.session.is_none() {
                continue;
            }
            let Ok(Some(task)) = self.store.get(&run.task_id).await else {
                continue;
            };
            let report = self.session_status_report(&run).await;
            let subject = format!("run:{}", run.id);
            self.record_status(&subject, &task.scope, &run.agent, report.status)
                .await;
            let action = block_action(
                &report,
                run.status,
                run.blocked_source,
                run.block_suspected_since.is_some(),
            );
            self.apply_block_action(&run, action).await;
        }
    }

    /// Carry out what `block_action` decided. Split from it so the decision
    /// stays a pure function -- see `occupancy::tests` -- while this half
    /// does the actual writing, journalling and publishing.
    async fn apply_block_action(&self, run: &Run, action: BlockAction) {
        match action {
            BlockAction::Nothing => {}
            BlockAction::Suspect => {
                self.patch_run(
                    run,
                    RunPatch {
                        block_suspected_since: Some(Utc::now()),
                        ..Default::default()
                    },
                )
                .await;
            }
            BlockAction::ClearSuspicion => {
                self.patch_run(
                    run,
                    RunPatch {
                        clear_block_suspicion: true,
                        ..Default::default()
                    },
                )
                .await;
            }
            // A hook told the runtime the agent is blocked, and nothing
            // already says so -- move the run (and, mirrored, the task)
            // into `Blocked`, sourced from the runtime rather than the
            // agent, so only this same poll noticing "working" again may
            // take it back out.
            BlockAction::Confirm => {
                let Some(updated) = self
                    .patch_run(
                        run,
                        RunPatch {
                            status: Some(RunStatus::Blocked),
                            blocked_since: Some(Utc::now()),
                            blocked_source: Some(BlockSource::Runtime),
                            clear_block_suspicion: true,
                            ..Default::default()
                        },
                    )
                    .await
                else {
                    return;
                };
                self.entry(
                    &run.task_id,
                    TaskEntry::new(
                        "daemon",
                        "blocked",
                        format!(
                            "{} reports the session is blocked and waiting for a human",
                            run.runtime
                        ),
                    )
                    .in_run(&run.id),
                )
                .await;
                self.mirror_to_task(&updated).await;
            }
            // The same hook that set this block says the session is active
            // again. Only fires when the daemon is the one holding the
            // block open (`blocked_source == Runtime`) -- an agent-set
            // block is untouched here no matter what the runtime says; see
            // `AGENTS.md` and issue #7.
            BlockAction::Unblock => {
                let Some(updated) = self
                    .patch_run(
                        run,
                        RunPatch {
                            status: Some(RunStatus::Running),
                            clear_blocked: true,
                            ..Default::default()
                        },
                    )
                    .await
                else {
                    return;
                };
                self.entry(
                    &run.task_id,
                    TaskEntry::new(
                        "daemon",
                        "unblocked",
                        format!("{} reports the session is active again", run.runtime),
                    )
                    .in_run(&run.id),
                )
                .await;
                self.mirror_to_task(&updated).await;
            }
        }
    }

    async fn patch_run(&self, run: &Run, patch: RunPatch) -> Option<Run> {
        match self.store.update_run(&run.id, &patch).await {
            Ok(updated) => {
                self.bus.publish(Event::RunUpdated { run: updated.clone() });
                Some(updated)
            }
            Err(e) => {
                tracing::warn!(run = %run.id, "could not record a block/unblock: {e}");
                None
            }
        }
    }

    /// How long this task usually takes, from its own finished runs. The median
    /// rather than the mean: one run that sat waiting for a human all night
    /// should not move the estimate for the rest.
    async fn historical_estimate_for(&self, task_id: &str) -> (Option<u64>, u32) {
        let runs = self.store.runs(task_id, 50).await.unwrap_or_default();
        let mut lengths: Vec<i64> = runs
            .iter()
            .filter(|r| r.status == RunStatus::Done)
            .filter_map(|r| r.ended_at.map(|end| (end - r.started_at).num_seconds()))
            .filter(|s| *s > 0)
            .collect();
        if lengths.is_empty() {
            return (None, 0);
        }
        lengths.sort_unstable();
        let samples = lengths.len() as u32;
        (Some(lengths[lengths.len() / 2] as u64), samples)
    }
}

/// The task's own estimate is the planning fact somebody deliberately wrote.
/// History only fills the gap when they did not write one.
fn plan_estimate(
    user: Option<u64>,
    historical: (Option<u64>, u32),
) -> (Option<u64>, u32, bool) {
    match user {
        Some(estimate) => (Some(estimate), 0, true),
        None => (historical.0, historical.1, false),
    }
}

/// What a fresh status report should do to a run's block state. A pure
/// decision, on purpose: no store, no runtime, so "an inferred block does not
/// change a status" and "an agent-set block is not cleared by the runtime
/// looking busy" are things a test can assert directly against this function
/// rather than against a fake `AgentRuntime` wired through `Engine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockAction {
    /// Move the run itself into `Blocked`, sourced from the runtime.
    Confirm,
    /// Not a status -- record the timestamp as a suspicion only.
    Suspect,
    /// A suspicion was recorded and no longer applies.
    ClearSuspicion,
    /// The daemon set this block and may honestly take it back.
    Unblock,
    /// Nothing to do. Covers, among other things, an agent-set block the
    /// runtime is not the one allowed to touch.
    Nothing,
}

/// `report` is what the runtime just said; `run_status`, `blocked_source` and
/// `suspected` are the run's own state going into this tick.
fn block_action(
    report: &StatusReport,
    run_status: RunStatus,
    blocked_source: Option<BlockSource>,
    suspected: bool,
) -> BlockAction {
    if report.status == RuntimeStatus::Blocked {
        return match report.source {
            // Hook authority. If the run is not already `Blocked` -- by
            // either source -- this is the report that makes it so. If it
            // already is, there is nothing left to set, only a suspicion
            // recorded moments earlier (before this same report caught up)
            // to let go of.
            StatusSource::Reported if run_status != RunStatus::Blocked => BlockAction::Confirm,
            StatusSource::Reported if suspected => BlockAction::ClearSuspicion,
            StatusSource::Reported => BlockAction::Nothing,
            // A screen's guess, or no attribution at all -- never a status,
            // only a suspicion, and only worth writing once.
            StatusSource::Inferred | StatusSource::Unknown if suspected => BlockAction::Nothing,
            StatusSource::Inferred | StatusSource::Unknown => BlockAction::Suspect,
        };
    }

    // Not `blocked` any more, by whatever measure. A standing suspicion is no
    // longer supported by anything and comes off first.
    if suspected {
        return BlockAction::ClearSuspicion;
    }

    // The runtime saying the session looks active again may only undo a
    // block the runtime itself put there. An agent-set block stands until
    // the agent's own next report says otherwise -- see `AGENTS.md`.
    let recovered = matches!(
        report.status,
        RuntimeStatus::Working | RuntimeStatus::Idle | RuntimeStatus::Starting
    );
    if recovered && run_status == RunStatus::Blocked && blocked_source == Some(BlockSource::Runtime) {
        return BlockAction::Unblock;
    }

    BlockAction::Nothing
}

fn block_of(run: &Run, title: Option<&str>, estimate_seconds: Option<u64>) -> OccupancyBlock {
    OccupancyBlock {
        run_id: run.id.clone(),
        task_id: run.task_id.clone(),
        title: title.unwrap_or("(deleted task)").to_string(),
        status: run.status.as_str().to_string(),
        trigger: run.trigger.as_str().to_string(),
        attempt: run.attempt,
        from: run.started_at,
        to: run.ended_at,
        estimate_seconds: if run.ended_at.is_none() {
            estimate_seconds
        } else {
            None
        },
    }
}

/// Seconds of the window a run held. Clipped to the window at both ends, so a
/// run that started yesterday counts only the part that is on the chart.
fn busy_seconds(blocks: &[OccupancyBlock], from: DateTime<Utc>, to: DateTime<Utc>) -> i64 {
    blocks
        .iter()
        .map(|b| {
            let start = b.from.max(from);
            let end = b.to.unwrap_or(to).min(to);
            (end - start).num_seconds().max(0)
        })
        .sum()
}

/// Drop everything before the window except the last state it was in, moved to
/// the window's edge. Without this an agent that has been idle since yesterday
/// draws nothing at all, which reads as "not observed".
fn trim_to_window(series: &mut Vec<StatusChange>, from: DateTime<Utc>) {
    let Some(last_before) = series.iter().rposition(|c| c.at < from) else {
        return;
    };
    series.drain(..last_before);
    if let Some(first) = series.first_mut() {
        if first.status != RuntimeStatus::Gone {
            first.at = from;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + secs, 0).unwrap()
    }

    fn change(status: RuntimeStatus, secs: i64) -> StatusChange {
        StatusChange {
            subject: "demo/a".into(),
            scope: "demo".into(),
            agent: "a".into(),
            status,
            at: at(secs),
        }
    }

    fn block(from: i64, to: Option<i64>) -> OccupancyBlock {
        OccupancyBlock {
            run_id: "r".into(),
            task_id: "t".into(),
            title: "t".into(),
            status: "done".into(),
            trigger: "manual".into(),
            attempt: 1,
            from: at(from),
            to: to.map(at),
            estimate_seconds: None,
        }
    }

    fn run(to: Option<i64>) -> Run {
        Run {
            id: "r".into(),
            task_id: "t".into(),
            attempt: 1,
            status: if to.is_some() {
                RunStatus::Done
            } else {
                RunStatus::Running
            },
            trigger: factory_core::run::Trigger::Manual,
            agent: "a".into(),
            adapter: "shell".into(),
            worktree_path: None,
            worktree_branch: None,
            runtime: "herdr".into(),
            session: None,
            token: None,
            result: None,
            error: None,
            started_at: at(0),
            ended_at: to.map(at),
            blocked_since: None,
            blocked_source: None,
            block_suspected_since: None,
        }
    }

    #[test]
    fn an_open_run_carries_the_tasks_estimate_but_a_finished_one_does_not() {
        assert_eq!(
            block_of(&run(None), Some("estimated"), Some(900)).estimate_seconds,
            Some(900)
        );
        assert_eq!(
            block_of(&run(Some(60)), Some("finished"), Some(900)).estimate_seconds,
            None
        );
    }

    #[test]
    fn an_exceeded_estimate_stays_fixed_instead_of_following_now() {
        let projected = block_of(&run(None), Some("slow"), Some(60));
        let expected_end =
            projected.from + Duration::seconds(projected.estimate_seconds.unwrap() as i64);
        assert!(expected_end < at(120));
        assert_eq!(projected.estimate_seconds, Some(60));
    }

    #[test]
    fn a_user_estimate_wins_over_history_for_a_scheduled_plan() {
        assert_eq!(
            plan_estimate(Some(900), (Some(120), 4)),
            (Some(900), 0, true)
        );
        assert_eq!(plan_estimate(None, (Some(120), 4)), (Some(120), 4, false));
        assert_eq!(plan_estimate(None, (None, 0)), (None, 0, false));
    }

    #[test]
    fn the_window_opens_in_the_state_it_was_already_in() {
        let mut series = vec![
            change(RuntimeStatus::Idle, 0),
            change(RuntimeStatus::Working, 50),
            change(RuntimeStatus::Idle, 500),
        ];
        trim_to_window(&mut series, at(100));
        assert_eq!(series.len(), 2);
        assert_eq!(series[0].status, RuntimeStatus::Working);
        assert_eq!(series[0].at, at(100));
    }

    #[test]
    fn nothing_before_the_window_leaves_the_series_alone() {
        let mut series = vec![change(RuntimeStatus::Idle, 200)];
        trim_to_window(&mut series, at(100));
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].at, at(200));
    }

    #[test]
    fn busy_time_is_clipped_to_the_window_at_both_ends() {
        // Started before the window, still running past the end of it.
        let seconds = busy_seconds(&[block(-100, None)], at(0), at(60));
        assert_eq!(seconds, 60);
    }

    #[test]
    fn an_open_run_counts_up_to_now() {
        assert_eq!(busy_seconds(&[block(10, None)], at(0), at(60)), 50);
    }

    fn report(status: RuntimeStatus, source: StatusSource) -> StatusReport {
        StatusReport { status, source }
    }

    #[test]
    fn a_hook_reported_block_moves_a_running_run_into_blocked() {
        let action = block_action(&report(RuntimeStatus::Blocked, StatusSource::Reported), RunStatus::Running, None, false);
        assert_eq!(action, BlockAction::Confirm);
    }

    #[test]
    fn an_inferred_block_does_not_change_a_status() {
        // Whatever state the run is actually in, a screen's guess never
        // produces `Confirm` or `Unblock` -- the only two actions that touch
        // `RunStatus`.
        for status in [RunStatus::Dispatching, RunStatus::Running, RunStatus::Blocked] {
            let action = block_action(&report(RuntimeStatus::Blocked, StatusSource::Inferred), status, None, false);
            assert_ne!(action, BlockAction::Confirm, "inferred must never confirm a block");
            assert_ne!(action, BlockAction::Unblock, "inferred must never unblock either");
        }
        // What it does instead is raise a suspicion, once.
        assert_eq!(
            block_action(&report(RuntimeStatus::Blocked, StatusSource::Inferred), RunStatus::Running, None, false),
            BlockAction::Suspect,
        );
        assert_eq!(
            block_action(&report(RuntimeStatus::Blocked, StatusSource::Inferred), RunStatus::Running, None, true),
            BlockAction::Nothing,
            "a suspicion already recorded is not re-raised every tick",
        );
    }

    #[test]
    fn an_unattributed_block_is_treated_as_a_guess_not_a_report() {
        // `Unknown` is what a runtime that never implemented `status_report`
        // hands back. It must not be trusted any more than `Inferred` is.
        let action = block_action(&report(RuntimeStatus::Blocked, StatusSource::Unknown), RunStatus::Running, None, false);
        assert_eq!(action, BlockAction::Suspect);
    }

    #[test]
    fn a_runtime_set_block_is_lifted_once_the_session_looks_active_again() {
        let action = block_action(
            &report(RuntimeStatus::Working, StatusSource::Unknown),
            RunStatus::Blocked,
            Some(BlockSource::Runtime),
            false,
        );
        assert_eq!(action, BlockAction::Unblock);
    }

    #[test]
    fn an_agent_set_block_is_not_cleared_by_the_runtime_looking_busy() {
        for working_status in [RuntimeStatus::Working, RuntimeStatus::Idle, RuntimeStatus::Starting] {
            let action = block_action(
                &report(working_status, StatusSource::Unknown),
                RunStatus::Blocked,
                Some(BlockSource::Agent),
                false,
            );
            assert_eq!(
                action,
                BlockAction::Nothing,
                "only the agent's own next report may clear a block it set"
            );
        }
    }

    #[test]
    fn a_cleared_suspicion_does_not_masquerade_as_an_unblock() {
        // The run was never actually put into `Blocked` by anything -- there
        // was only a suspicion -- so the runtime looking active again just
        // drops the suspicion, not a status nothing set.
        let action = block_action(
            &report(RuntimeStatus::Working, StatusSource::Unknown),
            RunStatus::Running,
            None,
            true,
        );
        assert_eq!(action, BlockAction::ClearSuspicion);
    }
}
