//! The occupancy chart, as a page (#193 phase 6, S12 part 9).
//!
//! **Page (D5).** The chart composes L4's runs, journal and liveness record with L3's roster of agents (through the
//! scope views) and the schedules tasks carry, and owns no state: it is drawn on demand. The run-liveness sampling and
//! the turn-end handling that feed it are L4's (`occupancy.rs`).
use crate::engine::Engine;
use crate::occupancy::{plan_estimate, planned_firings, trim_to_window};
use chrono::{DateTime, Duration, Utc};
use factory_core::error::Result;
use factory_core::occupancy::{spans_from, Occupancy, OccupancyBlock, OccupancyPlan, OccupancyRow, OccupancyScope, StatusChange};
use factory_core::task::TaskEntry;
use factory_process::occupancy_history::{block_of, blocked_segments, lay_out, window_bounds, TRANSITION_KINDS};
use std::collections::{BTreeMap, BTreeSet};

impl Engine {
    /// The chart over a window. With neither `from` nor `to` it is the one
    /// it has always been: `minutes` back from now and a quarter of that
    /// ahead. With both it is that window -- past, future, or across now --
    /// which is what lets a person pan and zoom. See [`window_bounds`].
    pub async fn occupancy(
        &self,
        minutes: Option<u32>,
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
    ) -> Result<Occupancy> {
        let now = Utc::now();
        let (from, to) = window_bounds(minutes, from, to, now)?;
        // What has happened stops at now, whichever side of it the window
        // is on. A window wholly in the future has no runs in it; asking the
        // store anyway would hand back every open run, drawn to a now that
        // is off the left-hand edge.
        let past_end = now.min(to);

        let runs = if from <= now {
            self.l4.store.runs_between(from, past_end).await?
        } else {
            Vec::new()
        };
        let tasks = self.l4.store.list(&Default::default()).await?;
        let factory = self.factory_snapshot();
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
            .map(|t| (t.id.as_str(), factory.canonical_scope_name(&t.scope)))
            .collect();
        // What each run waited on. The run record cannot say: `blocked_since`
        // is cleared the moment a run ends, so the journal is the only place a
        // finished run's blocked stretch survives (#121). One query for every
        // run on the chart, reaching back to the oldest one's start -- a run
        // open since before the window may have blocked before it, too.
        let on_chart: BTreeSet<&str> = runs.iter().map(|r| r.id.as_str()).collect();
        let mut transitions: BTreeMap<String, Vec<TaskEntry>> = BTreeMap::new();
        if let Some(earliest) = runs.iter().map(|r| r.started_at).min() {
            let since = earliest - Duration::seconds(1);
            for (_, entry) in self.l4.store.entries_of_kinds(TRANSITION_KINDS, since).await? {
                let Some(run_id) = entry.run_id.as_deref() else { continue };
                if on_chart.contains(run_id) {
                    transitions.entry(run_id.to_string()).or_default().push(entry);
                }
            }
        }

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
                    blocked_segments(
                        transitions.get(&run.id).map(Vec::as_slice).unwrap_or_default(),
                        run.ended_at,
                    ),
                ));
        }

        // What a schedule says is coming, drawn as wide as the task's own
        // history says it usually takes.
        let mut planned: BTreeMap<(String, String), Vec<OccupancyPlan>> = BTreeMap::new();
        for task in &tasks {
            let firings = planned_firings(task, now, from, to);
            if firings.is_empty() {
                continue;
            }
            let historical = if task.estimate_seconds.is_some() {
                (None, 0)
            } else {
                self.l4_service().historical_estimate_for(&task.id).await
            };
            let (estimate, samples, user_estimate) =
                plan_estimate(task.estimate_seconds, historical);
            let scope = scope_of.get(task.id.as_str()).cloned().unwrap_or_default();
            planned
                .entry((scope, task.agent.clone()))
                .or_default()
                .extend(firings.into_iter().map(|at| OccupancyPlan {
                    task_id: task.id.clone(),
                    title: task.title.clone(),
                    at,
                    estimate_seconds: estimate,
                    samples,
                    user_estimate,
                }));
        }

        // Liveness, grouped by the session it was observed on -- by subject,
        // never by agent. Two runs of the same agent at the same time are two
        // sessions with two independent states; interleaving them into one
        // series produces a strip that describes neither.
        //
        // Changes from before the window still matter: they say what state the
        // window opened in, so keep the last one before `from` as the opening
        // span.
        let changes = self.l4.store.status_changes(from - Duration::hours(24)).await?;
        let mut spans: BTreeMap<String, Vec<StatusChange>> = BTreeMap::new();
        for change in changes {
            spans.entry(change.subject.clone()).or_default().push(change);
        }
        for series in spans.values_mut() {
            trim_to_window(series, from);
        }

        let liveness_since = self.l4.store.status_origin().await?;

        let mut out = Vec::new();
        let (views, _available) = self.scope_views().await?;
        for view in views {
            let path = view.path.clone();
            let mut rows = Vec::new();
            for agent in &view.agents {
                let key = (view.name.clone(), agent.name.clone());
                let mut blocks = blocks.remove(&key).unwrap_or_default();
                let planned = planned.remove(&key).unwrap_or_default();
                // A row draws the standing agent's own session and nothing
                // else. A run's liveness is recorded too, but the run already
                // has a block on this row saying more than a screen can.
                let spans = spans
                    .remove(&format!("{}/{}", view.name, agent.name))
                    .map(|series| spans_from(&series, now))
                    .unwrap_or_default();
                let (busy_seconds, blocked_seconds, lanes, live) = lay_out(&mut blocks, from, past_end);
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
                    blocked_seconds,
                    lanes,
                    live,
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
        for ((scope, agent), mut blocks) in blocks {
            let (busy_seconds, blocked_seconds, lanes, live) = lay_out(&mut blocks, from, past_end);
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
                blocked_seconds,
                lanes,
                live,
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
}
