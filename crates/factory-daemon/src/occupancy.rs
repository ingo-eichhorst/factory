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
use factory_core::adapter::RuntimeStatus;
use factory_core::error::Result;
use factory_core::occupancy::{
    spans_from, Occupancy, OccupancyBlock, OccupancyPlan, OccupancyRow, OccupancyScope, StatusChange,
};
use factory_core::run::{Run, RunStatus};
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
                .push(block_of(run, titles.get(run.task_id.as_str()).copied()));
        }

        // What a schedule says is coming, drawn as wide as the task's own
        // history says it usually takes.
        let mut planned: BTreeMap<(String, String), Vec<OccupancyPlan>> = BTreeMap::new();
        for task in &tasks {
            let Some(at) = task.next_run_at else { continue };
            if at < now || at > to {
                continue;
            }
            let (estimate, samples) = self.estimate_for(&task.id).await;
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
    pub async fn record_run_liveness(self: &Arc<Self>) {
        let runs = self.store.active_runs().await.unwrap_or_default();
        for run in runs {
            if run.session.is_none() {
                continue;
            }
            let Ok(Some(task)) = self.store.get(&run.task_id).await else {
                continue;
            };
            let status = self.session_status(&run).await;
            let subject = format!("run:{}", run.id);
            self.record_status(&subject, &task.scope, &run.agent, status)
                .await;
        }
    }

    /// How long this task usually takes, from its own finished runs. The median
    /// rather than the mean: one run that sat waiting for a human all night
    /// should not move the estimate for the rest.
    async fn estimate_for(&self, task_id: &str) -> (Option<i64>, u32) {
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
        (Some(lengths[lengths.len() / 2]), samples)
    }
}

fn block_of(run: &Run, title: Option<&str>) -> OccupancyBlock {
    OccupancyBlock {
        run_id: run.id.clone(),
        task_id: run.task_id.clone(),
        title: title.unwrap_or("(deleted task)").to_string(),
        status: run.status.as_str().to_string(),
        trigger: run.trigger.as_str().to_string(),
        attempt: run.attempt,
        from: run.started_at,
        to: run.ended_at,
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
        }
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
}
