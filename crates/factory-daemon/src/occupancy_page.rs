//! The occupancy chart, as a page (#193 phase 6, S12 part 9).
//!
//! **Page (D5).** The chart composes L4's runs, journal and liveness record with L3's roster of agents (through the
//! scope views) and the schedules tasks carry, and owns no state: it is drawn on demand. The run-liveness sampling and
//! the turn-end handling that feed it are L4's (`occupancy.rs`). Standing sessions' uptime (#331) is L4's record of
//! what it read from L3's `StandingAgentObservationsFact`; the slots a row holds are admission's own count.
use crate::engine::Engine;
use crate::occupancy::{plan_estimate, planned_firings, trim_to_window};
use chrono::{DateTime, Duration, Utc};
use factory_core::agent::AgentSession;
use factory_core::error::Result;
use factory_core::occupancy::{
    spans_from, up_seconds, uptime_from, Occupancy, OccupancyBlock, OccupancyPlan, OccupancyRow, OccupancyScope,
    OccupancySession, OccupancySlots, StatusChange,
};
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
        // One handle on L4's service for everything the chart asks of it:
        // estimates, the liveness record and admission's slot count.
        let l4 = self.l4_service();

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
                l4.historical_estimate_for(&task.id).await
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
        // never by agent. Two sessions of the same agent at the same time have
        // two independent states; interleaving them into one series produces a
        // strip that describes neither, so each is a lane of its own (#331).
        // Which row a session belongs on is the (scope, agent) it was recorded
        // under, never a subject spelled out here: that is what keeps a second
        // instance of one agent on its row, whatever its id looks like.
        //
        // A run's liveness is recorded too, but the run already has a block on
        // its row saying more than a screen can.
        let (changes, liveness_since) = l4.liveness_record(from).await?;
        let mut sessions: BTreeMap<(String, String), BTreeMap<String, Vec<StatusChange>>> = BTreeMap::new();
        for change in changes {
            if change.subject.starts_with("run:") {
                continue;
            }
            let scope = factory.canonical_scope_name(&change.scope);
            // A session recorded before its scope's identity became its path
            // is the same session as the one after: one lane, not two.
            let subject = if scope != change.scope && change.subject == AgentSession::id_for(&change.scope, &change.agent) {
                AgentSession::id_for(&scope, &change.agent)
            } else {
                change.subject.clone()
            };
            sessions
                .entry((scope, change.agent.clone()))
                .or_default()
                .entry(subject)
                .or_default()
                .push(change);
        }

        let mut out = Vec::new();
        let (views, _available) = self.scope_views().await?;
        for view in views {
            let path = view.path.clone();
            let declared = factory.scope(&view.name).map(|s| s.declared_agents()).unwrap_or_default();
            let mut rows = Vec::new();
            for agent in &view.agents {
                let key = (view.name.clone(), agent.name.clone());
                let mut blocks = blocks.remove(&key).unwrap_or_default();
                let planned = planned.remove(&key).unwrap_or_default();
                // The roster knows one state per agent: it says how the
                // agent's own session ended, and nothing about another's.
                let stopped = agent.state == "stopped";
                let own = agent.id.as_deref();
                let sessions: Vec<OccupancySession> = sessions
                    .remove(&key)
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|(subject, mut series)| {
                        trim_to_window(&mut series, from);
                        let stopped = stopped && own.is_none_or(|id| id == subject);
                        let in_window = |start: DateTime<Utc>, end: Option<DateTime<Utc>>| {
                            start <= to && end.is_none_or(|end| end >= from)
                        };
                        let mut uptime = uptime_from(&series, stopped);
                        uptime.retain(|u| in_window(u.from, u.to));
                        if uptime.is_empty() {
                            return None;
                        }
                        let mut spans = spans_from(&series, now);
                        spans.retain(|s| in_window(s.from, s.to));
                        Some(OccupancySession { subject, uptime, spans })
                    })
                    .collect();
                let up_seconds = up_seconds(&sessions, from, past_end);
                // Admission's own count, the one `factory status` shows: a live
                // standing agent is a slot, each run with a session another.
                let max = declared.iter().find(|d| d.name() == agent.name).and_then(|d| d.max_sessions);
                let slots = if agent.lifetime != "task" || max.is_some() {
                    l4.capacity_for(&view.name, &agent.name, max)
                        .await
                        .ok()
                        .map(|cap| OccupancySlots { in_use: cap.in_use, max })
                } else {
                    None
                };
                let (busy_seconds, blocked_seconds, lanes, live) = lay_out(&mut blocks, from, past_end);
                rows.push(OccupancyRow {
                    agent: agent.name.clone(),
                    adapter: agent.adapter.clone(),
                    lifetime: agent.lifetime.clone(),
                    role: agent.role.clone(),
                    state: agent.state.clone(),
                    blocks,
                    planned,
                    sessions,
                    up_seconds,
                    slots,
                    busy_seconds,
                    blocked_seconds,
                    lanes,
                    live,
                });
            }
            // Standing agents first, each group in declaration order: the
            // always-on agents are what a person looks for first.
            rows.sort_by_key(|row| row.lifetime == "task");
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
                sessions: Vec::new(),
                up_seconds: 0,
                slots: None,
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

#[cfg(test)]
mod tests {
    //! The uptime layer (#331), end to end: L4's record of what it read from L3, assembled into rows by the page.
    use crate::agents::tests::recording_engine;
    use chrono::{Duration, Utc};
    use factory_core::adapter::runtime::RuntimeStatus;
    use factory_core::occupancy::{Occupancy, OccupancyRow, OccupancySlots, StatusChange, UptimeEnd};

    /// A task agent declared first and a permanent one after it, so the order the chart puts them in is its own.
    const SCOPE: &str = "name: demo\npath: .\nruntime: stub\nagents:\n  - name: builder\n    harness: configured\n  - name: watcher\n    harness: configured\n    lifetime: permanent\n    max_sessions: 5\n";

    fn change(subject: &str, status: RuntimeStatus, minutes_ago: i64) -> StatusChange {
        StatusChange {
            subject: subject.into(),
            scope: "demo".into(),
            agent: "watcher".into(),
            status,
            at: Utc::now() - Duration::minutes(minutes_ago),
        }
    }

    fn row<'a>(occ: &'a Occupancy, agent: &str) -> &'a OccupancyRow {
        occ.scopes
            .iter()
            .find(|s| s.name == "demo")
            .and_then(|s| s.rows.iter().find(|r| r.agent == agent))
            .unwrap_or_else(|| panic!("no row for {agent}: {occ:?}"))
    }

    #[tokio::test]
    async fn a_quiet_standing_agent_up_all_window_is_one_open_stretch_and_holds_its_slot() {
        let (engine, _stub, root) = recording_engine(SCOPE);
        engine.l3_service().start_agent("demo", "watcher").await.unwrap();
        // Idle for three days: its one row is far older than any window.
        engine.l4.store.append_status(&change("demo/watcher", RuntimeStatus::Idle, 3 * 24 * 60)).await.unwrap();

        let occ = engine.occupancy(Some(60), None, None).await.unwrap();
        let rows: Vec<_> = occ.scopes.iter().find(|s| s.name == "demo").unwrap().rows.iter().map(|r| r.agent.as_str()).collect();
        // Declared after `builder` (and after the scope's default agent), listed before both.
        assert_eq!(rows.first(), Some(&"watcher"), "standing rows come first: {rows:?}");
        assert!(rows.contains(&"builder"));

        let watcher = row(&occ, "watcher");
        assert_eq!(watcher.state, "ready");
        assert_eq!(watcher.sessions.len(), 1);
        assert_eq!(watcher.sessions[0].subject, "demo/watcher");
        let uptime = &watcher.sessions[0].uptime;
        assert_eq!(uptime.len(), 1, "{uptime:?}");
        assert_eq!((uptime[0].from, uptime[0].to, uptime[0].ended), (occ.from, None, None));
        assert_eq!(watcher.up_seconds, (occ.now - occ.from).num_seconds(), "up the whole of the window");
        assert_eq!(watcher.busy_seconds, 0, "uptime is never busy time");
        // `factory status`'s number: the live permanent agent is one slot of its own.
        assert_eq!(watcher.slots, Some(OccupancySlots { in_use: 1, max: Some(5) }));

        let builder = row(&occ, "builder");
        assert!(builder.sessions.is_empty() && builder.slots.is_none() && builder.up_seconds == 0);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_restart_and_a_persons_stop_end_the_stretches_they_ended() {
        let (engine, _stub, root) = recording_engine(SCOPE);
        engine.l3_service().start_agent("demo", "watcher").await.unwrap();
        for c in [
            change("demo/watcher", RuntimeStatus::Idle, 50),
            change("demo/watcher", RuntimeStatus::Gone, 40),
            change("demo/watcher", RuntimeStatus::Starting, 39),
            change("demo/watcher", RuntimeStatus::Working, 38),
        ] {
            engine.l4.store.append_status(&c).await.unwrap();
        }
        // A person stops it: L3 writes it down as gone, and L4 reads that as a fact.
        engine.l3_service().stop_agent("demo/watcher", &crate::l3_service::Asker {
            by: "the owner".into(),
            source: "owner",
        }).await.unwrap();
        engine.l4_service().sample_standing_agents().await;

        let occ = engine.occupancy(Some(60), None, None).await.unwrap();
        let watcher = row(&occ, "watcher");
        assert_eq!(watcher.state, "stopped");
        let ends: Vec<_> = watcher.sessions[0].uptime.iter().map(|u| u.ended).collect();
        assert_eq!(ends, [Some(UptimeEnd::Restarted), Some(UptimeEnd::Stopped)]);
        let up = watcher.up_seconds;
        assert!((48 * 60..=49 * 60 + 5).contains(&up), "10 minutes, then 38 up to the stop: {up}");
        assert_eq!(watcher.slots, Some(OccupancySlots { in_use: 0, max: Some(5) }), "a stopped agent holds nothing");
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn two_sessions_of_one_agent_are_two_lanes_and_one_uptime() {
        let (engine, _stub, root) = recording_engine(SCOPE);
        // However a second instance's id is spelled, it is recorded under its agent's (scope, name).
        for c in [
            change("demo/watcher", RuntimeStatus::Idle, 10),
            change("demo/watcher#2", RuntimeStatus::Working, 10),
            change("run:r1", RuntimeStatus::Working, 10),
        ] {
            engine.l4.store.append_status(&c).await.unwrap();
        }

        let occ = engine.occupancy(Some(60), None, None).await.unwrap();
        let watcher = row(&occ, "watcher");
        let lanes: Vec<_> = watcher.sessions.iter().map(|s| s.subject.as_str()).collect();
        assert_eq!(lanes, ["demo/watcher", "demo/watcher#2"], "a run's liveness is its block, not a lane");
        assert!(watcher.sessions.iter().all(|s| s.uptime.len() == 1 && s.spans.len() == 1));
        let up = watcher.up_seconds;
        assert!((10 * 60 - 5..=10 * 60 + 5).contains(&up), "side by side, each second counted once: {up}");
        std::fs::remove_dir_all(root).ok();
    }
}
