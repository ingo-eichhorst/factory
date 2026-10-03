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
use factory_core::adapter::agent::truncate_tail;
use factory_core::adapter::{RuntimeStatus, StatusReport, StatusSource};
use factory_core::error::{FactoryError, Result};
use factory_core::event::Event;
use factory_core::occupancy::{
    spans_from, Occupancy, OccupancyBlock, OccupancyPlan, OccupancyRow, OccupancyScope, OccupancySegment,
    StatusChange,
};
use factory_core::run::{BlockSource, FailKind, Run, RunPatch, RunStatus};
use factory_core::task::{TaskEntry, TurnEndEvent, TurnEnded};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::engine::Engine;
use crate::schedule;

/// How far back the chart looks when nobody says.
const DEFAULT_MINUTES: u32 = 12 * 60;
/// The widest dashboard/metrics preset. The chart still defaults to today,
/// but its authoritative interval unions also back the 90-day hour metrics.
const MAX_MINUTES: u32 = 90 * 24 * 60;
/// Nor is one narrower than this: a run is at least a pixel or two wide.
const MIN_MINUTES: u32 = 5;
/// How many firings of one schedule a window draws. An every-minute task
/// across a month is tens of thousands of bars nobody can tell apart; this
/// many already fills a wide chart edge to edge.
const MAX_FIRINGS: usize = 500;
/// The journal kinds that move a run into or out of `blocked`: `blocked`
/// itself, the daemon's `unblocked`, and every other run status an agent's
/// report is journalled under. Anything that ends the run without one of these
/// is covered by the run's `ended_at`.
const TRANSITION_KINDS: &[&str] = &[
    "blocked",
    "unblocked",
    "dispatching",
    "running",
    "verifying",
    "done",
    "failed",
    "cancelled",
];

impl Engine {
    /// The chart over a window. With neither `from` nor `to` it is the one
    /// it has always been: `minutes` back from now and a quarter of that
    /// ahead. With both it is that window -- past, future, or across now --
    /// which is what lets a person pan and zoom. See [`window_bounds`].
    pub async fn occupancy(
        self: &Arc<Self>,
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
            self.store.runs_between(from, past_end).await?
        } else {
            Vec::new()
        };
        let tasks = self.store.list(&Default::default()).await?;
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
            for (_, entry) in self.store.entries_of_kinds(TRANSITION_KINDS, since).await? {
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
                self.historical_estimate_for(&task.id).await
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
    /// saying working again), and, since issue #62, a hook-reported `idle`
    /// on a run nothing has reported the end of, are acted on. The scheduler
    /// has its own poll of the runtime for the `Gone` check, but that is an
    /// existing, unrelated call to plain `status` -- putting this logic here
    /// instead means `status_report`, the one call that can cost an extra
    /// subprocess (`herdr agent explain`), is asked for exactly once per run
    /// per tick, not two or three times over.
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

            // `turn_ended_action` is judged against `run.status` as this
            // loop found it, before `apply_block_action`'s patch above has
            // landed. That means a run this same poll's `block_action` just
            // `Unblock`ed -- it was `Blocked` a moment ago, and this same
            // `idle`+`Reported` report is what let it go -- cannot also
            // `Fail` in the same tick: `run.status` here still reads
            // `Blocked`, not `Running`. If the harness is still saying
            // `idle` on the very next poll, that one will fail it; a run is
            // never killed and reopened in the same breath, only across
            // two, the same way a `Blocked` run flipping twice needs two
            // polls of `block_action` to say so. A run whose earlier
            // *inferred* block just had its suspicion cleared above, by
            // contrast, is free to also `Fail` here in the same tick -- the
            // two are independent facts read off the same report, and
            // neither depends on the other.
            //
            // This only ever fires for a harness herdr itself treats as
            // hook-authoritative for `idle`, and today that is `pi` alone.
            // Verified live against `herdr agent explain --json`: a `pi`
            // pane answers `screen_detection_skipped: true`,
            // `screen_detection_skip_reason: "full_lifecycle_hook_authority"`;
            // a `claude` pane answers `screen_detection_skipped: false` with
            // a `matched_rule` instead, for every state including `idle` --
            // herdr is guessing from the screen for `claude-code`, and
            // `status_report` keeps answering `StatusSource::Inferred`, which
            // `turn_ended_action` deliberately never acts on.
            //
            // `claude-code` is covered by a second, independent path that
            // never goes through herdr at all: Claude Code's own `Stop` and
            // `StopFailure` hooks call the daemon directly the moment a turn
            // ends (`Engine::turn_ended`, issue #69). A `StopFailure` fails
            // the run there and then; a `Stop` is only held on the run, and
            // stands here, below, once `settle_turn_end` says it has.
            if turn_ended_action(&report, run.status) == TurnEndedAction::Fail {
                self.fail_run(&run.id, FailKind::TurnEnded, TURN_ENDED_REASON).await;
            } else if settle_turn_end(run.status, run.turn_ended_at, report.status, Utc::now()) {
                let why = run.turn_end_reason.as_deref().unwrap_or(TURN_ENDED_REASON);
                self.fail_run(&run.id, FailKind::TurnEnded, why).await;
            }
        }
    }

    /// A harness's lifecycle hook saying the agent's turn ended -- Claude
    /// Code's `Stop` or `StopFailure`, by way of `factory task turn-ended`.
    /// The harness speaking, not a guess about a terminal, so this may end
    /// a run on its own word (see `AGENTS.md`); what it decides is
    /// `hook_turn_ended_action`'s -- at once for `StopFailure`, and for
    /// `Stop` by way of `settle_turn_end` on a later liveness tick.
    ///
    /// A hook fires at the end of *every* turn, including the one in which
    /// the agent reported `done` -- so a task with no run in progress is the
    /// common case, not a mistake, and is answered quietly, with nothing
    /// journalled. The token is checked exactly as for a report: without it,
    /// anyone on the socket could end anyone's run.
    pub(crate) async fn turn_ended(self: &Arc<Self>, task_id: &str, turn: TurnEnded) -> Result<()> {
        let Some(run) = self.store.active_run(task_id).await? else {
            return Ok(());
        };
        self.check_run_token(&run, turn.token.as_deref(), task_id)?;
        // `#178`: Claude Code's own session id, when the hook payload names
        // one -- `--continue`'s fallback source for which session to resume,
        // kept even though this particular turn end may settle into nothing.
        if turn.session_id.is_some() {
            self.patch_run(&run, RunPatch { turn_ended_session_id: turn.session_id.clone(), ..Default::default() })
                .await;
        }
        let action = hook_turn_ended_action(&turn, run.status);
        // A turn ended, so the usage so far is worth a reading (#117) --
        // taken off this request, which is the harness's own hook waiting
        // on an answer. A run failing right here gets its run-end reading
        // from `close_session` instead.
        if !matches!(action, HookTurnAction::FailNow) {
            crate::costs::spawn_snapshot(self, run.clone(), factory_core::usage::SnapshotPoint::TurnEnded);
        }
        match action {
            HookTurnAction::FailNow => {
                self.fail_run(&run.id, FailKind::StopFailure, &hook_turn_ended_reason(&turn)).await;
            }
            HookTurnAction::Settle => {
                self.patch_run(
                    &run,
                    RunPatch {
                        turn_ended_at: Some(Utc::now()),
                        turn_end_reason: Some(hook_turn_ended_reason(&turn)),
                        ..Default::default()
                    },
                )
                .await;
            }
            HookTurnAction::Nothing => {}
        }
        Ok(())
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
                self.record_workflow_task_state(&run.task_id).await;
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
                self.record_workflow_task_state(&run.task_id).await;
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
                tracing::warn!(run = %run.id, "could not update the run from its runtime or harness: {e}");
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

/// Why `fail_run` is called below for a turn that ended without a report.
/// Pulled out to a constant, rather than written inline, so the exact text
/// -- issue #62's acceptance criteria is specifically about this string
/// saying what happened rather than reciting a timeout -- is something a
/// test can assert on directly, the same way `scheduler.rs`'s own overdue
/// reasons are tested by their callers.
const TURN_ENDED_REASON: &str = "its turn ended without reporting -- the harness said the \
    session went idle and nothing followed. Its session may still be there; look at the \
    transcript before starting it again.";

/// What a fresh status report says about whether a run's turn ended without
/// the agent ever calling back. Kept beside `block_action`, and for the
/// identical reason: only a hook-reported fact may move a run, a screen's
/// guess never may (see `AGENTS.md` and issue #62), and that is worth
/// pinning down in a test that needs no store, no runtime and no `Engine`.
///
/// Unlike `block_action`, an `Inferred` or `Unknown` idle here does not even
/// earn a suspicion. Ending a run outright is far less reversible than
/// moving it into `Blocked` -- a wrongly `Reported` block only pauses a run
/// for a human to look at, but a wrongly `Reported` idle would kill one
/// that is still working -- so the bar for acting is a plain fact from the
/// harness, not a guess with a timestamp attached. Nothing today shows a
/// `block_suspected_since`-shaped suspicion for this either, and inventing
/// display-only state for a guess this consequential is not worth doing
/// until something actually reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnEndedAction {
    /// The harness itself said the turn ended, and the run is still
    /// `Running` with no terminal report to show for it -- fail it now
    /// rather than waiting out `timeout_seconds`.
    Fail,
    Nothing,
}

/// Gated on `RunStatus::Running` on purpose, not `Dispatching` too, even
/// though the run that motivated issue #62 could in principle have died
/// waiting in either state: `Running` only happens once the agent's own
/// first report said `running` (see `AgentContext::reporting_contract`), so
/// a hook saying the turn ended here is a turn that genuinely started and
/// then stopped. A `Dispatching` run has not been handed anything to have a
/// turn about yet -- a launch that is merely slow to draw its first prompt
/// would look exactly like an idle pane to this check -- and it already
/// answers to `ack_timeout_seconds` (180s by default, against
/// `task_timeout_seconds`'s 3600s), a much shorter leash than the one this
/// issue is trying to shrink in the first place.
fn turn_ended_action(report: &StatusReport, run_status: RunStatus) -> TurnEndedAction {
    if run_status == RunStatus::Running
        && report.status == RuntimeStatus::Idle
        && report.source == StatusSource::Reported
    {
        return TurnEndedAction::Fail;
    }
    TurnEndedAction::Nothing
}

/// The same question as `turn_ended_action`, asked of the other path a
/// turn's end can arrive by: the harness's own lifecycle hook calling the
/// daemon directly (`Request::TaskTurnEnded`, Claude Code's `Stop` and
/// `StopFailure` -- issue #69), rather than a runtime relaying what a hook
/// told it. Kept a separate function rather than dressed up as a
/// `StatusReport` for that one to judge: `StatusSource` says where a
/// *runtime's* answer came from, and faking one would leave the herdr path's
/// tests unsure what they prove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HookTurnAction {
    /// The turn is over and nothing can bring it back: fail the run now.
    FailNow,
    /// The turn ended as far as this hook knows, but another hook may yet
    /// keep it going -- hold the fact on the run and let
    /// `settle_turn_end` decide on a later tick.
    Settle,
    Nothing,
}

/// What a turn-end hook means for a run in `run_status`.
///
/// * `StopFailure` fails the run at once. An API error ended the turn, and
///   Claude Code documents that no hook can block it -- the session handles
///   the error regardless -- so there is no turn left to continue.
/// * `Stop` is only held (`Settle`). Every `Stop` hook in a session runs on
///   the same event, and any of them may exit 2 to keep the turn going: a
///   scope's own "you have not run the tests yet" hook, say. Verified live:
///   Factory's hook fires, reports, and the turn carries on regardless.
///   Failing on the spot would kill a run that is still working.
/// * `Dispatching` counts, unlike in `turn_ended_action`. That check leaves
///   it out because a slow launch looks exactly like an idle pane; nothing
///   here is looking at a pane. A hook only fires after a turn, and a turn
///   only happens once the prompt is submitted, so a `Dispatching` run whose
///   turn ended is one whose agent answered without ever reporting
///   `running` -- or, likelier, whose very first API call failed.
/// * Pending background work wins. Claude Code ends a turn and wakes itself
///   again when a background task finishes or a session cron fires, so a
///   turn that ended with any of that pending has paused, not finished --
///   the hook's own payload says so (`background_tasks`, `session_crons`).
///   If that work never does wake it, no later turn ends to say so, and the
///   run falls back to `task_timeout_seconds` -- where it stood before this.
///
/// The same fallback, silently, where the hook cannot prove which run it
/// speaks for: with `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1`, Claude Code strips
/// `FACTORY_TASK_TOKEN` (and `FACTORY_TOKEN`) from every subprocess --
/// verified live -- so the call arrives with no token and is refused. The
/// agent's own `task report` loses its token the same way, so that setting
/// already breaks the reporting contract as a whole, not just this.
/// * `Blocked` stays out for the same reason as there: an agent that
///   reported `blocked` and ended its turn is doing exactly what it was
///   told, waiting for a human.
fn hook_turn_ended_action(turn: &TurnEnded, run_status: RunStatus) -> HookTurnAction {
    let open = matches!(run_status, RunStatus::Running | RunStatus::Dispatching);
    if !open || turn.pending_background > 0 {
        return HookTurnAction::Nothing;
    }
    match turn.event {
        TurnEndEvent::StopFailure => HookTurnAction::FailNow,
        TurnEndEvent::Stop => HookTurnAction::Settle,
    }
}

/// How long a `Stop` hook's turn end is held before it may stand. Long
/// enough for any other `Stop` hook to have started, and for a turn one of
/// them kept going to show as working on the next few ticks; the screen
/// check in `settle_turn_end` covers however long it then runs.
const STOP_SETTLE_SECONDS: i64 = 30;

/// Whether a held `Stop` turn end now stands, on a liveness tick. Three
/// things, all of them:
///
/// * the run is still open and nothing has reported since -- any report
///   from the agent clears `turn_ended_at`, and a `Stop` from a later turn
///   replaces it, restarting the wait;
/// * it has been held for `STOP_SETTLE_SECONDS`;
/// * the runtime reads the session as `idle`, by whatever means.
///
/// The last is a screen reading for `claude`, and `AGENTS.md` forbids
/// acting on one of those -- which this does not do. The harness's hook is
/// the fact that fails the run; the screen can only hold it back. A pane
/// that looks working, starting or blocked (a permission prompt in a turn
/// another hook kept going) keeps the run alive; only one that looks idle,
/// agreeing with what the harness said, lets the fact stand. The failure
/// this risks is the safe one: a pane misread as busy falls back to
/// `task_timeout_seconds`, which is where every run stood before this.
fn settle_turn_end(
    run_status: RunStatus,
    turn_ended_at: Option<DateTime<Utc>>,
    runtime_status: RuntimeStatus,
    now: DateTime<Utc>,
) -> bool {
    let open = matches!(run_status, RunStatus::Running | RunStatus::Dispatching);
    let held_long_enough = turn_ended_at
        .is_some_and(|at| now - at >= Duration::seconds(STOP_SETTLE_SECONDS));
    open && held_long_enough && runtime_status == RuntimeStatus::Idle
}

/// Why a run failed on a hook's word, naming which hook and, for an API
/// error, what the API said -- issue #62 asks for the reason to say what
/// happened, and "rate_limit" and "the turn just ended" call for different
/// next steps.
fn hook_turn_ended_reason(turn: &TurnEnded) -> String {
    let mut why = match turn.event {
        TurnEndEvent::Stop => "its turn ended without reporting -- the harness's Stop hook said \
            the turn finished, and no done, failed or blocked report came before it."
            .to_string(),
        TurnEndEvent::StopFailure => {
            let error = turn.error.as_deref().unwrap_or("unknown");
            let mut why = format!(
                "its turn ended on an API error ({error}) without reporting -- the harness's \
                 StopFailure hook said so"
            );
            if let Some(details) = turn.error_details.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
                why.push_str(&format!(": {details}"));
            }
            why.push('.');
            why
        }
    };
    why.push_str(
        " Its session may still be there; look at the transcript before starting it again.",
    );
    if let Some(last) = turn.last_message.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        why.push_str(&format!("\n\nIts last message:\n{}", truncate_tail(last, LAST_MESSAGE_BYTE_CAP)));
    }
    why
}

/// How much of the agent's last message a turn-end failure keeps -- the end,
/// since that is where it stopped. Capped again here, whatever the CLI sent:
/// this is what bounds the run's `error` field.
const LAST_MESSAGE_BYTE_CAP: usize = 2048;

fn block_of(
    run: &Run,
    title: Option<&str>,
    estimate_seconds: Option<u64>,
    segments: Vec<OccupancySegment>,
) -> OccupancyBlock {
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
        // Settled once the whole row is known, by `pack_lanes`. A run's
        // segments are part of its block, so they share its lane.
        lane: 0,
        segments,
    }
}

/// The stretches a run spent blocked, from its journal entries in journal
/// order. `blocked` opens one -- a second `blocked` while one is open is the
/// same wait, not a new one -- and whatever moves the run on closes it: the
/// daemon's `unblocked`, or any other status the agent reports. One still
/// open when the run ended closes at `ended_at`, whatever kind of entry the
/// ending wrote, and nothing reaches past it; one open on a run that has not
/// ended runs to now, and is sent without a `to`.
fn blocked_segments(entries: &[TaskEntry], ended_at: Option<DateTime<Utc>>) -> Vec<OccupancySegment> {
    let mut out: Vec<OccupancySegment> = Vec::new();
    let mut open: Option<DateTime<Utc>> = None;
    for entry in entries {
        match entry.kind.as_str() {
            "blocked" => {
                open.get_or_insert(entry.at);
            }
            kind if TRANSITION_KINDS.contains(&kind) => {
                if let Some(from) = open.take() {
                    out.push(OccupancySegment {
                        status: "blocked".into(),
                        from,
                        to: Some(entry.at),
                    });
                }
            }
            _ => {}
        }
    }
    if let Some(from) = open {
        out.push(OccupancySegment {
            status: "blocked".into(),
            from,
            to: ended_at,
        });
    }
    if let Some(end) = ended_at {
        for segment in &mut out {
            segment.to = segment.to.map(|to| to.min(end));
        }
    }
    // A block lifted in the same instant it was set held nothing up.
    out.retain(|s| s.to.map(|to| to > s.from).unwrap_or(true));
    out
}

/// Everything a row says about its blocks as a whole: how busy it was, how
/// much of that was spent blocked, how many lanes it needs, and how many runs
/// are still open. One call, so neither place that builds a row can lay its
/// blocks out and forget the rest.
fn lay_out(blocks: &mut [OccupancyBlock], from: DateTime<Utc>, now: DateTime<Utc>) -> (i64, i64, u32, u32) {
    let lanes = pack_lanes(blocks, now);
    let live = blocks.iter().filter(|b| b.to.is_none()).count() as u32;
    (busy_seconds(blocks, from, now), blocked_seconds(blocks, from, now), lanes, live)
}

/// Give every block a lane so that no two in one lane overlap, and say how many
/// lanes that took. Greedy interval packing: in start order, each block goes
/// into the first lane whose last block has ended by the time it starts -- a
/// run that begins the moment another ends shares its lane. An open run holds
/// its lane up to `now`. The blocks are left in start order.
///
/// The count comes from the blocks alone. An agent's `max_sessions` is parsed
/// and not enforced, so config cannot say how many runs will overlap.
fn pack_lanes(blocks: &mut [OccupancyBlock], now: DateTime<Utc>) -> u32 {
    blocks.sort_by(|a, b| a.from.cmp(&b.from).then_with(|| a.run_id.cmp(&b.run_id)));
    let mut ends: Vec<DateTime<Utc>> = Vec::new();
    for block in blocks.iter_mut() {
        let end = block.to.unwrap_or(now).max(block.from);
        let lane = match ends.iter().position(|&e| e <= block.from) {
            Some(free) => free,
            None => {
                ends.push(end);
                ends.len() - 1
            }
        };
        ends[lane] = end;
        block.lane = lane as u32;
    }
    ends.len().max(1) as u32
}

/// Seconds of the window at least one run held: the union of the blocks, so
/// runs side by side never count the same wall-clock second twice. Clipped to
/// the window at both ends, so a run that started yesterday counts only the
/// part that is on the chart.
fn busy_seconds(blocks: &[OccupancyBlock], from: DateTime<Utc>, to: DateTime<Utc>) -> i64 {
    union_seconds(blocks.iter().map(|b| (b.from, b.to)), from, to)
}

/// Seconds of the window at least one run was blocked, counted the way
/// `busy_seconds` counts runs: two runs waiting side by side wait one minute a
/// minute. A part of `busy_seconds`, never taken out of it.
fn blocked_seconds(blocks: &[OccupancyBlock], from: DateTime<Utc>, to: DateTime<Utc>) -> i64 {
    union_seconds(
        blocks.iter().flat_map(|b| b.segments.iter()).map(|s| (s.from, s.to)),
        from,
        to,
    )
}

/// The union of some intervals, clipped to `from..to`, in whole seconds. An
/// interval with no end runs to `to`.
fn union_seconds(
    intervals: impl Iterator<Item = (DateTime<Utc>, Option<DateTime<Utc>>)>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> i64 {
    let mut spans: Vec<(DateTime<Utc>, DateTime<Utc>)> = intervals
        .map(|(start, end)| (start.max(from), end.unwrap_or(to).min(to)))
        .filter(|(start, end)| end > start)
        .collect();
    spans.sort();
    let mut total = Duration::zero();
    let mut open: Option<(DateTime<Utc>, DateTime<Utc>)> = None;
    for (start, end) in spans {
        open = match open {
            Some((s, e)) if start <= e => Some((s, e.max(end))),
            Some((s, e)) => {
                total += e - s;
                Some((start, end))
            }
            None => Some((start, end)),
        };
    }
    if let Some((s, e)) = open {
        total += e - s;
    }
    total.num_seconds()
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

/// The firing a task's schedule will draw on the chart, if it has one in
/// `[now, to]`. A paused schedule keeps its slot but will not fire it, so
/// there is nothing coming to draw.
/// The chart's window, from what the request asked for.
///
/// Neither `from` nor `to`: `minutes` back from now (twelve hours when that
/// is absent too), and a quarter of it kept ahead of now -- without it the
/// next scheduled run is drawn on the right-hand edge, where it is a pixel
/// rather than a plan. This is the window the chart has always had.
///
/// Both: that window, as long as it is the right way round. Its width is
/// held between [`MIN_MINUTES`] and [`MAX_MINUTES`] by moving `from`, so the
/// right-hand edge a person dragged to stays where they put it. One without
/// the other is the fixed-width window beside it, `minutes` wide.
fn window_bounds(
    minutes: Option<u32>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    let minutes = minutes.unwrap_or(DEFAULT_MINUTES).clamp(MIN_MINUTES, MAX_MINUTES) as i64;
    let (from, to) = match (from, to) {
        (None, None) => {
            let ahead = (minutes / 4).max(1);
            return Ok((now - Duration::minutes(minutes), now + Duration::minutes(ahead)));
        }
        (Some(from), None) => (from, from + Duration::minutes(minutes)),
        (None, Some(to)) => (to - Duration::minutes(minutes), to),
        (Some(from), Some(to)) => (from, to),
    };
    if to <= from {
        return Err(FactoryError::BadRequest(format!(
            "an occupancy window ends after it begins; got from {} and to {}",
            from.to_rfc3339(),
            to.to_rfc3339()
        )));
    }
    let span = (to - from).clamp(
        Duration::minutes(MIN_MINUTES as i64),
        Duration::minutes(MAX_MINUTES as i64),
    );
    Ok((to - span, to))
}

/// Every firing of a task's schedule in the future part of the window. The
/// first is its `next_run_at`, the one firing the scheduler has actually
/// committed to; the rest follow from the schedule, so a person panning a
/// week ahead sees the week's runs and not just the next one. A paused
/// schedule plans nothing, and neither does one whose next firing is
/// already overdue -- that one is about to be a block, not a plan. A
/// `next_run_at` with no schedule behind it (a queued retry) is the one
/// firing it says and nothing after.
fn planned_firings(
    task: &factory_core::task::Task,
    now: DateTime<Utc>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Vec<DateTime<Utc>> {
    let Some(next) = task.next_run_at else { return Vec::new() };
    if task.schedule_paused || next < now {
        return Vec::new();
    }
    let from = from.max(now);
    match &task.schedule {
        Some(schedule) => schedule::firings_between(schedule, next, from, to, MAX_FIRINGS),
        None => (next >= from && next <= to).then_some(next).into_iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paused_schedule_plans_no_firing() {
        let now = Utc::now();
        let mut task = every_ten_minutes(now + Duration::minutes(10));
        let to = now + Duration::hours(1);
        let drawn = planned_firings(&task, now, now - Duration::hours(1), to);
        assert_eq!(drawn.first().copied(), task.next_run_at, "sanity: drawn while running");
        task.schedule_paused = true;
        assert!(planned_firings(&task, now, now - Duration::hours(1), to).is_empty());
        task.schedule_paused = false;
        task.next_run_at = Some(now + Duration::hours(2));
        assert!(planned_firings(&task, now, now - Duration::hours(1), to).is_empty(), "past the chart's edge");
    }

    fn every_ten_minutes(next: DateTime<Utc>) -> factory_core::task::Task {
        let mut task = factory_core::adapter::store::task_from_new(
            factory_core::task::NewTask {
                title: "every ten minutes".into(),
                schedule: Some(factory_core::task::Schedule::Every { seconds: 600 }),
                ..Default::default()
            },
            "demo".into(),
            "shell".into(),
            "herdr".into(),
        );
        task.next_run_at = Some(next);
        task
    }

    #[test]
    fn every_firing_in_the_future_part_of_the_window_is_planned_not_just_the_next() {
        let now = at(0);
        let task = every_ten_minutes(at(300));
        // An hour either side of now: 5, 15, 25, 35, 45 and 55 minutes ahead.
        let firings = planned_firings(&task, now, at(-3600), at(3600));
        assert_eq!(firings.len(), 6);
        assert_eq!(firings[0], at(300));
        assert_eq!(firings[5], at(3300));
        // Panned a day ahead, the same grid carries on.
        let tomorrow = planned_firings(&task, now, at(86_400), at(86_400 + 1800));
        assert_eq!(tomorrow, vec![at(86_700), at(87_300), at(87_900)]);
        // Panned into the past, nothing is scheduled: that is what blocks are for.
        assert!(planned_firings(&task, now, at(-7200), at(-3600)).is_empty());
    }

    #[test]
    fn an_overdue_or_unscheduled_next_run_is_drawn_as_it_always_was() {
        let now = at(0);
        let overdue = every_ten_minutes(at(-60));
        assert!(planned_firings(&overdue, now, at(-3600), at(3600)).is_empty(), "about to be a block");
        let mut retry = every_ten_minutes(at(900));
        retry.schedule = None;
        assert_eq!(planned_firings(&retry, now, at(-3600), at(3600)), vec![at(900)]);
        assert!(planned_firings(&retry, now, at(1000), at(3600)).is_empty());
    }

    #[test]
    fn with_no_window_given_the_chart_is_the_one_it_always_was() {
        let now = at(0);
        assert_eq!(window_bounds(Some(60), None, None, now).unwrap(), (at(-3600), at(900)));
        assert_eq!(
            window_bounds(None, None, None, now).unwrap(),
            (at(-12 * 3600), at(3 * 3600)),
            "twelve hours back, a quarter of that ahead"
        );
        assert_eq!(
            window_bounds(Some(1), None, None, now).unwrap(),
            (at(-300), at(60)),
            "minutes are still clamped"
        );
    }

    #[test]
    fn an_explicit_window_is_taken_as_given_past_future_or_across_now() {
        let now = at(0);
        let past = (at(-86_400 * 3), at(-86_400 * 2));
        assert_eq!(window_bounds(None, Some(past.0), Some(past.1), now).unwrap(), past);
        let future = (at(86_400), at(86_400 * 2));
        assert_eq!(window_bounds(Some(60), Some(future.0), Some(future.1), now).unwrap(), future, "minutes is ignored");
        // One edge alone is the other edge `minutes` away.
        assert_eq!(window_bounds(Some(60), Some(at(0)), None, now).unwrap(), (at(0), at(3600)));
        assert_eq!(window_bounds(Some(60), None, Some(at(0)), now).unwrap(), (at(-3600), at(0)));
    }

    #[test]
    fn an_explicit_window_is_clamped_by_moving_its_start() {
        let now = at(0);
        let (from, to) = window_bounds(None, Some(at(-86_400 * 90)), Some(at(3600)), now).unwrap();
        assert_eq!(to, at(3600), "the edge a person dragged to stays put");
        assert_eq!((to - from).num_minutes(), MAX_MINUTES as i64);
        let (from, to) = window_bounds(None, Some(at(0)), Some(at(10)), now).unwrap();
        assert_eq!((from, to), (at(10 - 300), at(10)), "never narrower than five minutes");
    }

    #[test]
    fn a_window_that_ends_before_it_begins_is_refused() {
        let now = at(0);
        for (from, to) in [(at(60), at(0)), (at(0), at(0))] {
            let err = window_bounds(None, Some(from), Some(to), now).unwrap_err();
            assert!(matches!(err, FactoryError::BadRequest(_)), "{err:?}");
        }
    }

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
            lane: 0,
            segments: Vec::new(),
        }
    }

    fn named(id: &str, from: i64, to: Option<i64>) -> OccupancyBlock {
        OccupancyBlock {
            run_id: id.into(),
            ..block(from, to)
        }
    }

    fn lanes_of(blocks: &[OccupancyBlock]) -> Vec<(String, u32)> {
        blocks.iter().map(|b| (b.run_id.clone(), b.lane)).collect()
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
            last_session: None,
            token: None,
            spent_token_sha256: None,
            superseded_token_sha256s: Vec::new(),
            continued_from: None,
            resumed_session: None,
            original_estimate: None,
            provider_account: None,
            re_estimate: None,
            result: None,
            routed_to: None,
            error: None,
            started_at: at(0),
            ended_at: to.map(at),
            blocked_since: None,
            blocked_source: None,
            block_suspected_since: None,
            turn_ended_at: None,
            turn_end_reason: None,
            turn_ended_session_id: None,
            required_steps: Vec::new(),
            usage: None,
            queued_at: None,
            scheduled_for: None,
            fail_kind: None,
        }
    }

    #[test]
    fn an_open_run_carries_the_tasks_estimate_but_a_finished_one_does_not() {
        assert_eq!(
            block_of(&run(None), Some("estimated"), Some(900), Vec::new()).estimate_seconds,
            Some(900)
        );
        assert_eq!(
            block_of(&run(Some(60)), Some("finished"), Some(900), Vec::new()).estimate_seconds,
            None
        );
    }

    #[test]
    fn an_exceeded_estimate_stays_fixed_instead_of_following_now() {
        let projected = block_of(&run(None), Some("slow"), Some(60), Vec::new());
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

    #[test]
    fn three_runs_side_by_side_are_one_busy_stretch_not_three() {
        // The #120 shape: three dispatched together and still going, after an
        // earlier run that finished on its own.
        let blocks = [
            block(0, Some(100)),
            block(200, None),
            block(200, None),
            block(201, None),
        ];
        assert_eq!(busy_seconds(&blocks, at(0), at(300)), 100 + 100);
    }

    #[test]
    fn overlapping_and_nested_runs_count_each_second_once() {
        // 0..50 and 30..80 overlap into 0..80; 10..20 sits inside it; 90..100
        // is apart. 80 + 10.
        let blocks = [
            block(30, Some(80)),
            block(0, Some(50)),
            block(10, Some(20)),
            block(90, Some(100)),
        ];
        assert_eq!(busy_seconds(&blocks, at(0), at(200)), 90);
        // And the window still clips the union.
        assert_eq!(busy_seconds(&blocks, at(40), at(95)), 40 + 5);
    }

    #[test]
    fn runs_one_after_another_all_share_the_first_lane() {
        let mut blocks = vec![
            named("c", 60, None),
            named("a", 0, Some(30)),
            // Starts the moment `a` ends: that is not an overlap.
            named("b", 30, Some(60)),
        ];
        assert_eq!(pack_lanes(&mut blocks, at(100)), 1);
        assert_eq!(
            lanes_of(&blocks),
            vec![("a".into(), 0), ("b".into(), 0), ("c".into(), 0)],
            "a row without overlap is drawn exactly as before, in start order"
        );
    }

    #[test]
    fn three_concurrent_runs_get_three_lanes() {
        let mut blocks = vec![
            named("r119", 2, None),
            named("r116", -100, Some(-10)),
            named("r117", 0, None),
            named("r118", 1, None),
        ];
        assert_eq!(pack_lanes(&mut blocks, at(60)), 3);
        assert_eq!(
            lanes_of(&blocks),
            vec![
                ("r116".into(), 0),
                ("r117".into(), 0),
                ("r118".into(), 1),
                ("r119".into(), 2),
            ]
        );
    }

    #[test]
    fn a_freed_lane_is_reused_before_a_new_one_is_opened() {
        let mut blocks = vec![
            named("long", 0, Some(100)),
            named("short", 10, Some(20)),
            // Lane 1 is free again at 30; lane 0 is still taken.
            named("next", 30, Some(40)),
            // Both taken at 35 -- `next` holds lane 1 until 40.
            named("third", 35, Some(50)),
        ];
        assert_eq!(pack_lanes(&mut blocks, at(200)), 3);
        assert_eq!(
            lanes_of(&blocks),
            vec![
                ("long".into(), 0),
                ("short".into(), 1),
                ("next".into(), 1),
                ("third".into(), 2),
            ]
        );
    }

    #[test]
    fn an_open_run_holds_its_lane_up_to_now() {
        // Still running at 50, so a run that started at 40 cannot share it.
        let mut blocks = vec![named("open", 0, None), named("later", 40, Some(45))];
        assert_eq!(pack_lanes(&mut blocks, at(50)), 2);
        assert_eq!(blocks[1].lane, 1);
    }

    #[test]
    fn a_row_with_no_runs_still_has_one_lane() {
        assert_eq!(pack_lanes(&mut [], at(0)), 1);
    }

    #[test]
    fn a_rows_layout_counts_what_is_still_open() {
        let mut blocks = vec![
            named("done", 0, Some(10)),
            named("a", 20, None),
            named("b", 20, None),
        ];
        let (busy, blocked, lanes, live) = lay_out(&mut blocks, at(0), at(30));
        assert_eq!((busy, blocked, lanes, live), (10 + 10, 0, 2, 2));
    }

    // -- blocked segments (#121) --------------------------------------------

    fn entry(source: &str, kind: &str, secs: i64) -> TaskEntry {
        let mut e = TaskEntry::new(source, kind, kind).in_run("r");
        e.at = at(secs);
        e
    }

    fn blocked(from: i64, to: Option<i64>) -> OccupancySegment {
        OccupancySegment {
            status: "blocked".into(),
            from: at(from),
            to: to.map(at),
        }
    }

    #[test]
    fn a_run_that_blocked_until_it_was_done_keeps_the_wait_after_it_ends() {
        // #121's own run: 4m28s of a 4m39s run spent waiting on a human.
        let journal = [
            entry("daemon", "dispatched", 0),
            entry("agent", "running", 8),
            entry("agent", "blocked", 11),
            entry("agent", "done", 279),
        ];
        assert_eq!(blocked_segments(&journal, Some(at(279))), vec![blocked(11, Some(279))]);
    }

    #[test]
    fn each_block_and_resume_is_its_own_segment() {
        let journal = [
            entry("agent", "running", 0),
            entry("agent", "blocked", 10),
            entry("agent", "running", 40),
            entry("agent", "blocked", 60),
            entry("agent", "running", 70),
            entry("agent", "done", 100),
        ];
        assert_eq!(
            blocked_segments(&journal, Some(at(100))),
            vec![blocked(10, Some(40)), blocked(60, Some(70))]
        );
    }

    #[test]
    fn a_block_the_daemon_set_is_closed_by_the_daemon_lifting_it() {
        let journal = [
            entry("agent", "running", 0),
            entry("daemon", "blocked", 5),
            entry("daemon", "unblocked", 25),
            entry("agent", "done", 30),
        ];
        assert_eq!(blocked_segments(&journal, Some(at(30))), vec![blocked(5, Some(25))]);
    }

    #[test]
    fn a_run_still_blocked_has_a_segment_open_to_now() {
        let journal = [entry("agent", "running", 0), entry("agent", "blocked", 5)];
        assert_eq!(blocked_segments(&journal, None), vec![blocked(5, None)]);
        // ...and the row counts it up to now, as it counts the run.
        let mut blocks = vec![OccupancyBlock {
            segments: blocked_segments(&journal, None),
            ..block(0, None)
        }];
        let (busy, blocked_secs, _, _) = lay_out(&mut blocks, at(0), at(30));
        assert_eq!((busy, blocked_secs), (30, 25));
    }

    #[test]
    fn a_run_with_no_transitions_has_no_segments() {
        assert!(blocked_segments(&[], Some(at(60))).is_empty());
        let journal = [entry("agent", "running", 0), entry("agent", "note", 5), entry("agent", "done", 9)];
        assert!(blocked_segments(&journal, Some(at(9))).is_empty());
        // Nothing in the payload either: the block is exactly what it was.
        let json = serde_json::to_value(block(0, Some(9))).unwrap();
        assert!(json.get("segments").is_none());
    }

    #[test]
    fn a_second_blocked_report_is_the_same_wait_not_a_new_one() {
        let journal = [
            entry("agent", "blocked", 5),
            entry("agent", "note", 8),
            entry("agent", "blocked", 12),
            entry("agent", "running", 20),
        ];
        assert_eq!(blocked_segments(&journal, None), vec![blocked(5, Some(20))]);
    }

    #[test]
    fn a_run_that_ended_while_blocked_stops_waiting_when_it_ended() {
        // Cancelled, timed out, or failed by the daemon -- whatever the
        // ending journalled, `ended_at` closes the wait, and nothing written
        // afterwards stretches it.
        let journal = [entry("agent", "blocked", 5), entry("daemon", "failed", 95)];
        assert_eq!(blocked_segments(&journal, Some(at(90))), vec![blocked(5, Some(90))]);
        assert_eq!(
            blocked_segments(&[entry("agent", "blocked", 5)], Some(at(90))),
            vec![blocked(5, Some(90))]
        );
    }

    #[test]
    fn blocked_time_is_reported_beside_busy_time_not_taken_out_of_it() {
        let mut waiting = named("a", 0, Some(100));
        waiting.segments = vec![blocked(10, Some(60))];
        let mut also_waiting = named("b", 50, Some(100));
        also_waiting.segments = vec![blocked(50, Some(80))];
        let mut blocks = vec![waiting, also_waiting];
        let (busy, blocked_secs, lanes, _) = lay_out(&mut blocks, at(0), at(200));
        // Busy is unchanged by the waits; two runs waiting at once wait once.
        assert_eq!(busy, 100);
        assert_eq!(blocked_secs, 70);
        assert_eq!(lanes, 2);
        // Clipped to the window like busy time.
        assert_eq!(blocked_seconds(&blocks, at(20), at(70)), 50);
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
    fn a_hook_reported_idle_ends_a_running_turn() {
        let action = turn_ended_action(&report(RuntimeStatus::Idle, StatusSource::Reported), RunStatus::Running);
        assert_eq!(action, TurnEndedAction::Fail);
    }

    #[test]
    fn a_guessed_idle_never_ends_a_turn() {
        // The whole point of issue #62: only the harness's own word may end
        // a run early. A screen that merely looks idle must never be acted
        // on, and there is nothing here that even shows it as a suspicion.
        for source in [StatusSource::Inferred, StatusSource::Unknown] {
            let action = turn_ended_action(&report(RuntimeStatus::Idle, source), RunStatus::Running);
            assert_eq!(action, TurnEndedAction::Nothing, "{source:?} must never end a turn");
        }
    }

    #[test]
    fn a_reported_idle_outside_running_changes_nothing() {
        // `Dispatching` answers to its own, much shorter `ack_timeout_seconds`;
        // the terminal statuses have no turn left to end.
        for status in [
            RunStatus::Dispatching,
            RunStatus::Blocked,
            RunStatus::Done,
            RunStatus::Failed,
            RunStatus::Cancelled,
        ] {
            let action = turn_ended_action(&report(RuntimeStatus::Idle, StatusSource::Reported), status);
            assert_eq!(action, TurnEndedAction::Nothing, "{status:?} must not be failed by this check");
        }
    }

    #[test]
    fn only_a_reported_idle_ends_a_turn_no_other_reported_status_does() {
        for status in [
            RuntimeStatus::Working,
            RuntimeStatus::Starting,
            RuntimeStatus::Blocked,
            RuntimeStatus::Gone,
            RuntimeStatus::Unknown,
        ] {
            let action = turn_ended_action(&report(status, StatusSource::Reported), RunStatus::Running);
            assert_eq!(action, TurnEndedAction::Nothing, "{status:?} is not a turn having ended");
        }
    }

    #[test]
    fn the_turn_ended_reason_says_what_happened_not_a_timeout() {
        // Issue #62's acceptance criteria in a single assertion: the reason
        // a run failed with names the event, not a duration.
        assert!(TURN_ENDED_REASON.contains("turn ended without reporting"), "{TURN_ENDED_REASON}");
        assert!(!TURN_ENDED_REASON.contains("no report in"), "{TURN_ENDED_REASON}");
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

    // -- the harness's own turn-end hook (issue #69) ------------------------

    fn turn(event: TurnEndEvent, pending_background: u32) -> TurnEnded {
        TurnEnded {
            event,
            pending_background,
            error: None,
            error_details: None,
            last_message: None,
            token: None,
            session_id: None,
        }
    }

    #[test]
    fn a_stop_failure_fails_a_run_still_waiting_on_a_report_at_once() {
        // No hook can block a StopFailure, so there is no turn to continue.
        for status in [RunStatus::Running, RunStatus::Dispatching] {
            assert_eq!(
                hook_turn_ended_action(&turn(TurnEndEvent::StopFailure, 0), status),
                HookTurnAction::FailNow,
                "{status:?}"
            );
        }
    }

    #[test]
    fn a_stop_is_only_held_because_another_hook_may_keep_the_turn_going() {
        for status in [RunStatus::Running, RunStatus::Dispatching] {
            assert_eq!(
                hook_turn_ended_action(&turn(TurnEndEvent::Stop, 0), status),
                HookTurnAction::Settle,
                "{status:?}"
            );
        }
    }

    #[test]
    fn a_turn_that_ended_with_background_work_pending_only_paused() {
        for event in [TurnEndEvent::Stop, TurnEndEvent::StopFailure] {
            assert_eq!(
                hook_turn_ended_action(&turn(event, 1), RunStatus::Running),
                HookTurnAction::Nothing,
                "the harness will wake the agent again; {event:?}"
            );
        }
    }

    #[test]
    fn a_hook_reported_turn_end_leaves_a_blocked_or_finished_run_alone() {
        // Blocked is the agent doing what it was told: waiting for a human.
        for event in [TurnEndEvent::Stop, TurnEndEvent::StopFailure] {
            for status in [RunStatus::Blocked, RunStatus::Done, RunStatus::Failed, RunStatus::Cancelled] {
                assert_eq!(
                    hook_turn_ended_action(&turn(event, 0), status),
                    HookTurnAction::Nothing,
                    "{event:?} on {status:?}"
                );
            }
        }
    }

    #[test]
    fn a_held_stop_stands_once_it_is_old_enough_and_the_session_looks_idle() {
        let ended = at(0);
        let later = at(STOP_SETTLE_SECONDS);
        assert!(settle_turn_end(RunStatus::Running, Some(ended), RuntimeStatus::Idle, later));
        assert!(settle_turn_end(RunStatus::Dispatching, Some(ended), RuntimeStatus::Idle, later));
    }

    #[test]
    fn a_held_stop_waits_out_its_settling_time() {
        let early = at(STOP_SETTLE_SECONDS - 1);
        assert!(!settle_turn_end(RunStatus::Running, Some(at(0)), RuntimeStatus::Idle, early));
    }

    #[test]
    fn a_session_that_looks_busy_holds_a_stop_back_but_never_triggers_one() {
        let later = at(STOP_SETTLE_SECONDS * 10);
        // Another hook kept the turn going: working, or stopped at a
        // permission prompt, or still coming up -- or the runtime cannot
        // say. None of those lets the harness's word stand yet.
        for status in [
            RuntimeStatus::Working,
            RuntimeStatus::Blocked,
            RuntimeStatus::Starting,
            RuntimeStatus::Unknown,
            RuntimeStatus::Gone,
        ] {
            assert!(!settle_turn_end(RunStatus::Running, Some(at(0)), status, later), "{status:?}");
        }
        // And an idle screen with no hook behind it is never enough.
        assert!(!settle_turn_end(RunStatus::Running, None, RuntimeStatus::Idle, later));
    }

    #[test]
    fn a_held_stop_does_not_outlive_the_run_being_blocked_or_over() {
        let later = at(STOP_SETTLE_SECONDS * 10);
        for status in [RunStatus::Blocked, RunStatus::Done, RunStatus::Failed, RunStatus::Cancelled] {
            assert!(!settle_turn_end(status, Some(at(0)), RuntimeStatus::Idle, later), "{status:?}");
        }
    }

    #[test]
    fn the_hook_reasons_say_which_hook_and_what_the_api_said() {
        let stop = hook_turn_ended_reason(&turn(TurnEndEvent::Stop, 0));
        assert!(stop.contains("turn ended without reporting"), "{stop}");
        assert!(stop.contains("Stop hook"), "{stop}");
        assert!(!stop.contains("no report in"), "a turn ending is not a timeout: {stop}");

        let mut failure = turn(TurnEndEvent::StopFailure, 0);
        failure.error = Some("server_error".into());
        failure.error_details = Some("Connection reset by peer".into());
        failure.last_message = Some("Now I will write the fix".into());
        let why = hook_turn_ended_reason(&failure);
        assert!(why.contains("API error (server_error)"), "{why}");
        assert!(why.contains("Connection reset by peer"), "{why}");
        assert!(why.ends_with("Now I will write the fix"), "the last message closes it: {why}");
    }

    #[test]
    fn a_huge_last_message_is_capped_in_the_reason() {
        let mut long = turn(TurnEndEvent::Stop, 0);
        long.last_message = Some(format!("{}the end", "y".repeat(LAST_MESSAGE_BYTE_CAP * 4)));
        let why = hook_turn_ended_reason(&long);
        assert!(why.len() < LAST_MESSAGE_BYTE_CAP + 500, "{} bytes", why.len());
        assert!(why.ends_with("the end"));
    }
}
