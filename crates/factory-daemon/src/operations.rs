//! Where the Operations report is served, and the few actions it offers
//! (`#106`, the L4 Operations tab and `factory stats`). Like `goals/mod.rs`
//! and `scenarios/mod.rs`: `factory_core::operations` is the pure model,
//! tested on its own, and decides what every number and exception means;
//! this module gathers what it reads from the store, the journal and the
//! config, and nothing else. It holds no state of its own -- the report is
//! computed on read, like `/api/production`, and nothing here keeps a copy.
//!
//! ## What is read, and how much
//!
//! * **Runs**: `TaskStore::runs_between` over [`HISTORY_DAYS`] (or two
//!   windows, if that is longer) -- every open run, since an open run
//!   overlaps any window that reaches now, plus every run that ended inside
//!   it. That one query is the global run listing the projection needs:
//!   enough finished runs for the pace percentiles, both health windows and
//!   a recovery that began before the current one. A failure older than the
//!   history is not in the dead set; it is history, not a thing to act on.
//! * **Open runs' journals**: `run_entries` per open run, which there are
//!   only ever a handful of -- the newest entry is the run's last word, and
//!   a blocked run's newest `blocked` entry is its reason, in its agent's
//!   own words (or the runtime's, for a block a lifecycle hook reported).
//! * **Journal-only facts**: one `TaskStore::entries_of_kinds` query for
//!   `schedule_skipped` and `answer` entries, so nothing walks every task's
//!   whole journal, transcripts and all. A passed-over slot needs a look
//!   for [`MISSED_LOOKBACK_HOURS`]; after that it is history, and the
//!   journal still has it.
//! * **Standing agents** as the store has them; the model judges only a
//!   permanent agent, and only on whether its session is there.
//! * **Signposts** through `Engine::triggered_signposts`, the cheap half of
//!   the Scenarios report, and only for the unscoped report: a signpost
//!   watches a company-wide metric and belongs to a scenario, not a scope.
//!   A scenario directory that cannot be read costs the report its
//!   signposts, never the report itself.
//!
//! ## Capacity is unknown, and says so
//!
//! Nothing in Factory limits how many sessions a scope may hold. The
//! `max_sessions` key older configs carry is read and thrown away
//! (`config.rs`, "It has no effect now"), so there is no number to hold the
//! sessions in use against. [`OperationsInput::capacity`] stays empty and
//! every `Flow::sessions_max` is absent -- unknown, not a limit of zero --
//! rather than showing a figure that nothing enforces.
//!
//! ## Actions
//!
//! Every action the tab offers is a request that already existed, or a
//! narrow one beside it, and each is journaled with who asked and why
//! ([`Asked`]): run again (`task.run`), cancel (`task.cancel`), pause and
//! resume (`task.update`'s `schedule_paused`), skip the next slot
//! (`task.skip_next`, [`Engine::skip_next`]) and answer a blocked run
//! (`run.answer`, [`Engine::answer_run`]). Nothing here starts, stops or
//! changes anything on its own -- a picture, not a controller (design §8).

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use factory_core::error::{FactoryError, Result};
use factory_core::operations::{
    self, HealthWindow, OperationsInput, OperationsReport, SkippedSlots, TriggeredSignpost,
};
use factory_core::run::RunStatus;
use factory_core::task::{Task, TaskEntry, TaskFilter, TaskPatch};

use crate::access::Caller;
use crate::engine::Engine;
use crate::schedule;

/// How far back the report reads finished runs when the window asks for
/// less: the pace percentiles and the dead set want more history than a
/// seven-day window and its predecessor hold.
const HISTORY_DAYS: i64 = 60;

/// How long a passed-over slot stays in the attention queue.
const MISSED_LOOKBACK_HOURS: i64 = 24;

/// How much of an open run's journal is read for its last word and its
/// block reason. An open run has not written its transcript yet, and a
/// block reason older than this many lines has been talked over since.
const OPEN_RUN_ENTRIES: u32 = 50;

/// The journal kind of an answer typed into a blocked run -- what the
/// report counts as an intervention.
pub(crate) const ANSWER_KIND: &str = "answer";

/// Who asked for an action and why, as the journal records it: the entry's
/// `source` says whether a person or an agent asked, `data.by` which one,
/// and `data.reason` -- when one was given -- why.
pub(crate) struct Asked {
    pub by: String,
    pub source: &'static str,
    pub reason: Option<String>,
}

impl Asked {
    pub(crate) fn new(caller: &Caller, reason: Option<String>) -> Self {
        Self {
            by: caller.describe(),
            source: match caller {
                Caller::Owner => "owner",
                Caller::Agent { .. } => "agent",
            },
            // A reason of nothing but spaces is no reason.
            reason: reason.map(|r| r.trim().to_string()).filter(|r| !r.is_empty()),
        }
    }

    /// `"by the owner"`, `"by the owner: the host is being moved"`.
    pub(crate) fn words(&self) -> String {
        match &self.reason {
            Some(reason) => format!("by {}: {reason}", self.by),
            None => format!("by {}", self.by),
        }
    }

    /// `data` for the entry: who, why, and whatever the action adds.
    pub(crate) fn data(&self, mut extra: serde_json::Value) -> serde_json::Value {
        if let Some(map) = extra.as_object_mut() {
            map.insert("by".into(), self.by.clone().into());
            if let Some(reason) = &self.reason {
                map.insert("reason".into(), reason.clone().into());
            }
        }
        extra
    }

    /// One journal line for an action this caller asked for.
    pub(crate) fn entry(&self, kind: &str, message: String, extra: serde_json::Value) -> TaskEntry {
        TaskEntry::new(self.source, kind, message).with_data(self.data(extra))
    }
}

impl Engine {
    /// `Request::Operations`: the whole report for `scope` (by its own name,
    /// or every scope) over `window`. See the module doc for what is read.
    pub(crate) async fn operations_report(
        self: &Arc<Self>,
        scope: Option<&str>,
        window: HealthWindow,
    ) -> Result<OperationsReport> {
        let now = Utc::now();
        let snapshot = self.factory_snapshot();
        // Resolved, so a bare path still finds its scope and a typo is
        // refused rather than answered with an empty report.
        let scope = scope.map(|s| snapshot.scope(s).map(|s| s.name.clone())).transpose()?;

        let history = Duration::days(HISTORY_DAYS.max(2 * window.days()));
        let runs = self.store.runs_between(now - history, now).await?;
        let tasks = self.store.list(&TaskFilter::default()).await?;
        let agents = self.store.agents().await?;
        let scope_of: BTreeMap<&str, &str> = tasks.iter().map(|t| (t.id.as_str(), t.scope.as_str())).collect();
        let in_scope = |task_id: &str| match &scope {
            None => true,
            Some(s) => scope_of.get(task_id) == Some(&s.as_str()),
        };

        let mut block_reasons = BTreeMap::new();
        let mut last_progress = BTreeMap::new();
        for run in runs.iter().filter(|r| !r.status.is_terminal() && in_scope(&r.task_id)) {
            let entries = self.store.run_entries(&run.id, OPEN_RUN_ENTRIES).await?;
            if let Some(last) = entries.last() {
                last_progress.insert(run.id.clone(), last.at);
            }
            if run.status == RunStatus::Blocked {
                if let Some(said) = entries.iter().rev().find(|e| e.kind == "blocked") {
                    block_reasons.insert(run.id.clone(), said.message.clone());
                }
            }
        }

        // Answers are counted over both health windows; slots only over the
        // lookback. One query for both, from the earlier of the two.
        let answers_from = now - Duration::days(2 * window.days());
        let missed_from = now - Duration::hours(MISSED_LOOKBACK_HOURS);
        let journal = self
            .store
            .entries_of_kinds(&["schedule_skipped", ANSWER_KIND], answers_from.min(missed_from))
            .await?;
        let mut skipped = Vec::new();
        let mut answers = Vec::new();
        for (task_id, entry) in journal.into_iter().filter(|(t, _)| in_scope(t)) {
            match entry.kind.as_str() {
                ANSWER_KIND if entry.at > answers_from => answers.push(entry.at),
                "schedule_skipped" if entry.at > missed_from => {
                    if let Some(slots) = skipped_slots(task_id, &entry) {
                        skipped.push(slots);
                    }
                }
                _ => {}
            }
        }

        let signposts = if scope.is_none() {
            match self.triggered_signposts(now).await {
                Ok(found) => found.into_iter().map(signpost).collect(),
                Err(e) => {
                    tracing::warn!("operations: leaving scenario signposts out: {e}");
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        let input = OperationsInput {
            now,
            tasks: &tasks,
            runs: &runs,
            agents: &agents,
            scope: scope.as_deref(),
            window,
            capacity: BTreeMap::new(),
            block_reasons,
            last_progress,
            skipped,
            signposts,
            answers,
            // Two ticks: a slot the next tick is about to fire is not late
            // -- what the model's own default means, at this instance's tick.
            late_after_seconds: Some(2 * snapshot.config.daemon.tick_seconds as i64),
        };
        Ok(operations::report(&input))
    }

    /// `Request::TaskSkipNext`: pass over a scheduled task's next slot. The
    /// schedule moves to the first slot after it -- or after now, when the
    /// slot has already passed undispatched, so skipping a late slot never
    /// turns into firing an earlier one. A retry queued for the task is the
    /// next thing that would fire, so it is what gets skipped: the streak is
    /// dropped, the same way resuming a schedule drops one, and the journal
    /// says so.
    pub(crate) async fn skip_next(&self, id: &str, asked: &Asked) -> Result<Task> {
        let task = self.require(id).await?;
        let Some(s) = &task.schedule else {
            return Err(FactoryError::BadRequest("only a scheduled task has a next slot to skip".into()));
        };
        if task.schedule_paused {
            return Err(FactoryError::BadRequest(
                "the schedule is paused, so no slot is coming to skip; resume it first".into(),
            ));
        }
        let Some(slot) = task.next_run_at else {
            return Err(FactoryError::BadRequest("the task has no next slot to skip".into()));
        };
        let now = Utc::now();
        let next = schedule::next_after(s, slot.max(now))?;
        let dropped_retry = task.pending_retry.is_some();
        let updated = self
            .store
            .update(
                id,
                &TaskPatch {
                    next_run_at: Some(next),
                    clear_pending_retry: dropped_retry,
                    ..Default::default()
                },
            )
            .await?;
        let what = if dropped_retry { "the queued retry" } else { "the slot" };
        self.entry(
            id,
            asked.entry(
                "slot_skipped",
                format!(
                    "{what} at {} skipped {}; next firing at {}",
                    slot.to_rfc3339(),
                    asked.words(),
                    next.to_rfc3339()
                ),
                serde_json::json!({ "skipped": slot, "next": next, "dropped_retry": dropped_retry }),
            ),
        )
        .await;
        self.bus.publish(factory_core::event::Event::TaskUpdated { task: updated.clone() });
        Ok(updated)
    }

    /// `Request::RunAnswer`: type `text` into a blocked run's own session and
    /// press enter. Refused for a run that is not `Blocked` -- an answer to a
    /// question nobody is asking lands in whatever the agent is doing -- and
    /// for one with no session, which Factory did not open or has already
    /// closed. Standing agents' sessions are never reached from here: the
    /// id is a run's. The reason is required and journaled as an
    /// intervention; the text is not, since a record is no place for what
    /// a person had to type. The run stays `Blocked` until its agent says
    /// otherwise.
    pub(crate) async fn answer_run(&self, run_id: &str, text: &str, asked: &Asked) -> Result<()> {
        if asked.reason.is_none() {
            return Err(FactoryError::BadRequest(
                "an answer needs a reason: it is journaled as an intervention".into(),
            ));
        }
        if text.trim().is_empty() {
            return Err(FactoryError::BadRequest("an answer needs some text to type".into()));
        }
        let run = self.require_run(run_id).await?;
        if run.status != RunStatus::Blocked {
            return Err(FactoryError::BadRequest(format!(
                "run {run_id} is {}, not blocked; only a run waiting for a human is answered",
                run.status.as_str()
            )));
        }
        if run.session.is_none() {
            return Err(FactoryError::BadRequest(format!("run {run_id} has no session to answer into")));
        }
        self.run_input(run_id, Some(text), &["enter".to_string()]).await?;
        self.entry(
            &run.task_id,
            asked
                .entry(
                    ANSWER_KIND,
                    format!("answered {}", asked.words()),
                    serde_json::json!({ "chars": text.chars().count() }),
                )
                .in_run(&run.id),
        )
        .await;
        Ok(())
    }
}

/// A `schedule_skipped` entry read back into the model's shape. One the
/// scheduler wrote without its data -- none should exist -- is left out
/// rather than guessed at.
fn skipped_slots(task_id: String, entry: &TaskEntry) -> Option<SkippedSlots> {
    let data = entry.data.as_ref()?;
    let time = |key: &str| data.get(key).and_then(|v| serde_json::from_value::<DateTime<Utc>>(v.clone()).ok());
    Some(SkippedSlots {
        task_id,
        at: entry.at,
        count: data.get("count")?.as_u64()? as u32,
        first: time("first")?,
        last: time("last")?,
        reason: data.get("reason").and_then(|v| v.as_str()).map(str::to_string),
    })
}

fn signpost(t: factory_core::protocol::TriggeredSignpost) -> TriggeredSignpost {
    TriggeredSignpost {
        scenario: t.scenario,
        metric: t.metric,
        reason: t.reason,
    }
}

#[cfg(test)]
mod tests {
    //! Engine-level Operations reports and actions, on a temporary instance
    //! with a real sqlite store -- not `factory_core::operations` itself
    //! (covered on its own), but this module's gathering and journaling,
    //! each exception kind end to end the way a real request reaches it.

    use super::*;
    use factory_core::adapter::{AgentRuntime, StartRequest, TaskStore};
    use factory_core::agent::{AgentSession, AgentState, Lifetime};
    use factory_core::config::{Config, DaemonConfig, Factory, Instance, Scope};
    use factory_core::operations::ExceptionKind;
    use factory_core::protocol::{Payload, Request, Response};
    use factory_core::role::Role;
    use factory_core::run::{FailKind, NewRun, Run, RunPatch, Trigger};
    use factory_core::task::{NewTask, Schedule, SessionRef};
    use factory_plugins::{Registry, SqliteStore};
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// A runtime that opens nothing and remembers what was typed into it.
    #[derive(Default)]
    struct TypingRuntime {
        typed: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl AgentRuntime for TypingRuntime {
        fn name(&self) -> &str {
            "quiet"
        }
        async fn start(&self, req: &StartRequest) -> Result<SessionRef> {
            Ok(SessionRef { runtime: "quiet".into(), handle: req.id.clone(), meta: Default::default() })
        }
        async fn submit(&self, _: &SessionRef, _: &str) -> Result<()> {
            Ok(())
        }
        async fn status(&self, _: &SessionRef) -> Result<factory_core::adapter::RuntimeStatus> {
            Ok(factory_core::adapter::RuntimeStatus::Working)
        }
        async fn send_text(&self, _: &SessionRef, text: &str) -> Result<()> {
            self.typed.lock().unwrap().push(text.to_string());
            Ok(())
        }
        async fn send_keys(&self, _: &SessionRef, keys: &[String]) -> Result<()> {
            self.typed.lock().unwrap().extend(keys.iter().map(|k| format!("<{k}>")));
            Ok(())
        }
        async fn read(&self, _: &SessionRef, _: u32) -> Result<String> {
            Ok(String::new())
        }
        async fn stop(&self, _: &SessionRef) -> Result<()> {
            Ok(())
        }
    }

    fn scope(id: &str, name: &str, path: PathBuf) -> Scope {
        let mut scope: Scope = serde_yaml_ng::from_str(&format!("id: {id}\nname: {name}\nruntime: quiet\n")).unwrap();
        scope.path = path;
        scope
    }

    /// Two scopes, `demo` and `other`, on a store in memory, with
    /// [`TypingRuntime`] as the runtime every run's session belongs to.
    fn test_engine() -> (Arc<Engine>, Arc<TypingRuntime>, PathBuf) {
        let root = std::env::temp_dir().join(format!("factory-operations-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("projects/demo")).unwrap();
        std::fs::create_dir_all(root.join("projects/other")).unwrap();
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig { power_assertion: false, ..DaemonConfig::default() },
            scope: None,
            scopes: vec![
                scope("demo-id", "demo", root.join("projects/demo")),
                scope("other-id", "other", root.join("projects/other")),
            ],
            roles: Default::default(),
            policies: Default::default(),
            infrastructure: Default::default(),
            plugins_dir: None,
        };
        let runtime = Arc::new(TypingRuntime::default());
        let mut registry = Registry::with_builtins();
        registry.add_runtime(runtime.clone(), "test");
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        let engine = Arc::new(Engine::new(Factory { root: root.clone(), config }, registry, store, PathBuf::from("factory"), Vec::new()));
        (engine, runtime, root)
    }

    async fn task_in(engine: &Engine, scope: &str, title: &str, schedule: Option<Schedule>) -> Task {
        engine
            .create(NewTask {
                title: title.into(),
                instructions: "true".into(),
                scope: Some(scope.into()),
                agent: Some("shell".into()),
                worktree: Some(false),
                schedule,
                ..Default::default()
            })
            .await
            .unwrap()
    }

    async fn run_of(engine: &Engine, task_id: &str) -> Run {
        engine
            .store
            .create_run(&NewRun {
                task_id: task_id.into(),
                trigger: Trigger::Manual,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: "quiet".into(),
                token: "tok".into(),
                queued_at: Some(Utc::now()),
                scheduled_for: None,
            })
            .await
            .unwrap()
    }

    fn session(run: &Run) -> SessionRef {
        SessionRef { runtime: "quiet".into(), handle: run.id.clone(), meta: Default::default() }
    }

    /// A run of `task_id`, blocked, with its agent's reason in the journal.
    async fn blocked_run(engine: &Engine, task_id: &str, why: &str) -> Run {
        let run = run_of(engine, task_id).await;
        let run = engine
            .store
            .update_run(
                &run.id,
                &RunPatch {
                    status: Some(RunStatus::Blocked),
                    session: Some(session(&run)),
                    blocked_since: Some(Utc::now()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        engine.entry(task_id, TaskEntry::new("agent", "blocked", why).in_run(&run.id)).await;
        run
    }

    fn kinds(report: &OperationsReport) -> Vec<ExceptionKind> {
        report.attention.iter().map(|e| e.kind).collect()
    }

    fn owner(reason: &str) -> Asked {
        Asked::new(&Caller::Owner, Some(reason.into()))
    }

    // -- the exception kinds, end to end ------------------------------------

    #[tokio::test]
    async fn a_blocked_run_is_reported_in_its_agents_own_words() {
        let (engine, _, root) = test_engine();
        let task = task_in(&engine, "demo", "needs a key", None).await;
        let run = blocked_run(&engine, &task.id, "I need the staging API key").await;

        let report = engine.operations_report(None, HealthWindow::Week).await.unwrap();
        let e = report.attention.iter().find(|e| e.kind == ExceptionKind::Blocked).expect("blocked");
        assert_eq!(e.run_id.as_deref(), Some(run.id.as_str()));
        assert_eq!(e.reason, "I need the staging API key");
        assert!(e.actions.contains(&operations::Action::Answer));
        assert_eq!(report.flow[0].wip.blocked, 1);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_run_the_runtime_suspects_is_a_suspicion_never_a_status() {
        let (engine, _, root) = test_engine();
        let task = task_in(&engine, "demo", "quiet one", None).await;
        let run = run_of(&engine, &task.id).await;
        engine
            .store
            .update_run(
                &run.id,
                &RunPatch {
                    status: Some(RunStatus::Running),
                    block_suspected_since: Some(Utc::now()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let report = engine.operations_report(None, HealthWindow::Week).await.unwrap();
        let e = report.attention.iter().find(|e| e.kind == ExceptionKind::SuspectedStuck).expect("suspected");
        assert!(e.suspicion);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_failure_with_no_retry_left_is_in_the_dead_set() {
        let (engine, _, root) = test_engine();
        let task = task_in(&engine, "demo", "breaks", None).await;
        let run = run_of(&engine, &task.id).await;
        engine.fail_run(&run.id, FailKind::AgentFailed, "exit 1").await;

        let report = engine.operations_report(None, HealthWindow::Week).await.unwrap();
        let e = report.attention.iter().find(|e| e.kind == ExceptionKind::FailedExhausted).expect("exhausted");
        assert!(e.reason.contains("exit 1"), "{}", e.reason);
        assert_eq!(e.actions, vec![operations::Action::RunAgain]);
        assert_eq!(report.health.current.fail_by_kind.get(&FailKind::AgentFailed), Some(&1));
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_run_past_the_p95_of_its_task_is_aging() {
        let (engine, _, root) = test_engine();
        let task = task_in(&engine, "demo", "usually instant", None).await;
        // Five runs that ended the moment they started: a p95 of zero.
        for _ in 0..operations::MIN_HISTORY {
            let run = run_of(&engine, &task.id).await;
            engine
                .store
                .update_run(
                    &run.id,
                    &RunPatch { status: Some(RunStatus::Done), ended_at: Some(run.started_at), ..Default::default() },
                )
                .await
                .unwrap();
        }
        let open = run_of(&engine, &task.id).await;
        engine
            .store
            .update_run(&open.id, &RunPatch { status: Some(RunStatus::Running), ..Default::default() })
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let report = engine.operations_report(None, HealthWindow::Week).await.unwrap();
        let e = report.attention.iter().find(|e| e.run_id.as_deref() == Some(open.id.as_str())).expect("the open run");
        assert_eq!(e.kind, ExceptionKind::Aging);
        let item = report.aging.items.iter().find(|i| i.run_id.as_deref() == Some(open.id.as_str())).unwrap();
        assert_eq!(item.basis, operations::PaceBasis::Task);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn an_overdue_slot_is_late_and_a_passed_over_one_is_missed() {
        let (engine, _, root) = test_engine();
        let late = task_in(&engine, "demo", "late", Some(Schedule::Every { seconds: 3600 })).await;
        engine
            .store
            .update(&late.id, &TaskPatch { next_run_at: Some(Utc::now() - Duration::hours(1)), ..Default::default() })
            .await
            .unwrap();

        // Fired an hour late on a one-minute schedule: the scheduler writes
        // down the slots between, exactly as a real late tick would.
        let missed = task_in(&engine, "demo", "missed", Some(Schedule::Every { seconds: 60 })).await;
        let missed = engine
            .store
            .update(&missed.id, &TaskPatch { next_run_at: Some(Utc::now() - Duration::hours(1)), ..Default::default() })
            .await
            .unwrap();
        engine.advance_schedule(&missed).await.unwrap();

        let report = engine.operations_report(None, HealthWindow::Week).await.unwrap();
        let of = |id: &str| report.attention.iter().filter(|e| e.task_id.as_deref() == Some(id)).map(|e| e.kind).collect::<Vec<_>>();
        assert_eq!(of(&late.id), vec![ExceptionKind::ScheduleLate]);
        assert_eq!(of(&missed.id), vec![ExceptionKind::ScheduleMissed]);
        let row = report.schedules.iter().find(|r| r.task_id == missed.id).unwrap();
        assert!(row.skipped > 0);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_permanent_agent_whose_session_is_gone_has_lost_liveness() {
        let (engine, _, root) = test_engine();
        let agent = |name: &str, lifetime, state| AgentSession {
            id: AgentSession::id_for("demo", name),
            name: name.into(),
            scope: "demo".into(),
            agent: "shell".into(),
            runtime: "quiet".into(),
            lifetime,
            role: Role::worker(),
            assigned_role: None,
            state,
            token: None,
            session: None,
            attach: None,
            declared: true,
            error: None,
            started_at: Utc::now(),
            last_seen_at: Utc::now(),
        };
        engine.store.put_agent(&agent("keeper", Lifetime::Permanent, AgentState::Gone)).await.unwrap();
        engine.store.put_agent(&agent("temp", Lifetime::Temporary, AgentState::Gone)).await.unwrap();
        engine.store.put_agent(&agent("fine", Lifetime::Permanent, AgentState::Ready)).await.unwrap();

        let report = engine.operations_report(None, HealthWindow::Week).await.unwrap();
        let lost: Vec<&str> = report
            .attention
            .iter()
            .filter(|e| e.kind == ExceptionKind::LivenessLost)
            .filter_map(|e| e.agent.as_deref())
            .collect();
        assert_eq!(lost, vec!["keeper"], "only a permanent agent, only for its session");
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_triggered_signpost_is_an_observation_on_the_unscoped_report_only() {
        let (engine, _, root) = test_engine();
        std::fs::create_dir_all(root.join(".factory/scenarios")).unwrap();
        std::fs::write(
            root.join(".factory/scenarios/slow-year.yaml"),
            "name: slow-year\ntitle: A slow year\nsignposts:\n  - { metric: throughput_week, below: 999 }\n",
        )
        .unwrap();

        let report = engine.operations_report(None, HealthWindow::Week).await.unwrap();
        let e = report.attention.iter().find(|e| e.kind == ExceptionKind::TriggeredSignpost).expect("signpost");
        assert!(e.observation);
        assert_eq!(e.title.as_deref(), Some("slow-year"));
        // The full Scenarios report says the same thing.
        let full = engine.scenarios_report(None).await.unwrap();
        assert_eq!(full.triggered.len(), 1);

        let scoped = engine.operations_report(Some("demo"), HealthWindow::Week).await.unwrap();
        assert!(!kinds(&scoped).contains(&ExceptionKind::TriggeredSignpost));
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_scope_narrows_every_part_and_an_unknown_one_is_refused() {
        let (engine, _, root) = test_engine();
        let here = task_in(&engine, "demo", "here", None).await;
        let there = task_in(&engine, "other", "there", None).await;
        blocked_run(&engine, &here.id, "here").await;
        blocked_run(&engine, &there.id, "there").await;

        let report = engine.operations_report(Some("demo"), HealthWindow::Month).await.unwrap();
        assert_eq!(report.scope.as_deref(), Some("demo"));
        assert!(report.attention.iter().all(|e| e.scope.as_deref() == Some("demo")), "{:?}", report.attention);
        assert_eq!(report.flow.len(), 1);
        assert_eq!(report.health.window, HealthWindow::Month);
        assert_eq!(report.flow[0].sessions_max, None, "no limit is declared anywhere, so none is shown");

        assert!(engine.operations_report(Some("nowhere"), HealthWindow::Week).await.is_err());
        std::fs::remove_dir_all(root).ok();
    }

    // -- the actions, and what each one journals ---------------------------

    fn entry_of<'a>(entries: &'a [TaskEntry], kind: &str) -> &'a TaskEntry {
        entries.iter().rev().find(|e| e.kind == kind).unwrap_or_else(|| panic!("no {kind} entry in {entries:?}"))
    }

    fn data_str<'a>(entry: &'a TaskEntry, key: &str) -> Option<&'a str> {
        entry.data.as_ref().and_then(|d| d.get(key)).and_then(|v| v.as_str())
    }

    #[tokio::test]
    async fn skip_next_moves_the_schedule_one_slot_and_journals_who_and_why() {
        let (engine, _, root) = test_engine();
        let task = task_in(&engine, "demo", "hourly", Some(Schedule::Every { seconds: 3600 })).await;
        let slot = task.next_run_at.unwrap();

        let skipped = engine.skip_next(&task.id, &owner("the host is being moved")).await.unwrap();
        let next = skipped.next_run_at.unwrap();
        assert!(next > slot, "past the skipped slot");
        assert!(next <= slot + Duration::seconds(3600) + Duration::seconds(5), "and no further than the one after it");

        let entries = engine.store.entries(&task.id, 50).await.unwrap();
        let e = entry_of(&entries, "slot_skipped");
        assert_eq!(e.source, "owner");
        assert_eq!(data_str(e, "by"), Some("the owner"));
        assert_eq!(data_str(e, "reason"), Some("the host is being moved"));
        assert!(!entries.iter().any(|e| e.kind == "schedule_skipped"), "a decision is not a missed slot");

        let one_off = task_in(&engine, "demo", "one-off", None).await;
        let err = engine.skip_next(&one_off.id, &owner("x")).await.unwrap_err();
        assert!(err.to_string().contains("only a scheduled task"), "{err}");
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn skipping_a_late_slot_never_fires_an_earlier_one() {
        let (engine, _, root) = test_engine();
        let task = task_in(&engine, "demo", "hourly", Some(Schedule::Every { seconds: 3600 })).await;
        engine
            .store
            .update(&task.id, &TaskPatch { next_run_at: Some(Utc::now() - Duration::hours(5)), ..Default::default() })
            .await
            .unwrap();
        let skipped = engine.skip_next(&task.id, &owner("stale")).await.unwrap();
        assert!(skipped.next_run_at.unwrap() > Utc::now(), "from now, not from the overdue slot");
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn an_answer_types_into_the_blocked_runs_own_session_and_journals_an_intervention() {
        let (engine, runtime, root) = test_engine();
        let task = task_in(&engine, "demo", "needs a key", None).await;
        let run = blocked_run(&engine, &task.id, "which key?").await;

        let response = engine
            .handle_request(Request::RunAnswer {
                id: run.id.clone(),
                text: "use the staging one".into(),
                reason: "it asked which key".into(),
            })
            .await;
        assert!(matches!(response, Response::Ok { .. }), "{response:?}");
        assert_eq!(*runtime.typed.lock().unwrap(), vec!["use the staging one".to_string(), "<enter>".to_string()]);

        let entries = engine.store.run_entries(&run.id, 50).await.unwrap();
        let e = entry_of(&entries, ANSWER_KIND);
        assert_eq!(data_str(e, "by"), Some("the owner"));
        assert_eq!(data_str(e, "reason"), Some("it asked which key"));
        assert!(!e.message.contains("staging"), "the text itself is not journaled: {}", e.message);
        assert_eq!(
            engine.require_run(&run.id).await.unwrap().status,
            RunStatus::Blocked,
            "whether it is unblocked is the agent's to say"
        );

        let report = engine.operations_report(None, HealthWindow::Week).await.unwrap();
        assert_eq!(report.health.current.interventions, 1, "an answer is an intervention");
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn an_answer_is_refused_without_a_reason_or_a_blocked_run() {
        let (engine, runtime, root) = test_engine();
        let task = task_in(&engine, "demo", "t", None).await;
        let blocked = blocked_run(&engine, &task.id, "?").await;
        let err = engine.answer_run(&blocked.id, "yes", &Asked::new(&Caller::Owner, Some("   ".into()))).await.unwrap_err();
        assert!(err.to_string().contains("needs a reason"), "{err}");

        engine
            .store
            .update_run(&blocked.id, &RunPatch { status: Some(RunStatus::Running), clear_blocked: true, ..Default::default() })
            .await
            .unwrap();
        let err = engine.answer_run(&blocked.id, "yes", &owner("r")).await.unwrap_err();
        assert!(err.to_string().contains("not blocked"), "{err}");
        assert!(runtime.typed.lock().unwrap().is_empty(), "nothing typed on a refusal");
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn run_cancel_and_pause_journal_who_asked_and_why() {
        let (engine, _, root) = test_engine();
        let task = task_in(&engine, "demo", "weekly", Some(Schedule::Every { seconds: 3600 })).await;

        let run = run_of(&engine, &task.id).await;
        let response = engine
            .handle_request(Request::TaskCancel { id: task.id.clone(), reason: Some("wrong branch".into()) })
            .await;
        assert!(matches!(response, Response::Ok { .. }), "{response:?}");
        let entries = engine.store.run_entries(&run.id, 50).await.unwrap();
        let e = entry_of(&entries, "cancel_requested");
        assert_eq!(data_str(e, "reason"), Some("wrong branch"));

        let response = engine
            .handle_request(Request::TaskUpdate {
                id: task.id.clone(),
                patch: TaskPatch { schedule_paused: Some(true), ..Default::default() },
                reason: Some("stop the line".into()),
            })
            .await;
        assert!(matches!(response, Response::Ok { data: Payload::Task { .. } }), "{response:?}");
        let entries = engine.store.entries(&task.id, 50).await.unwrap();
        let e = entry_of(&entries, "schedule_paused");
        assert_eq!(e.source, "owner");
        assert_eq!(data_str(e, "reason"), Some("stop the line"));
        assert!(e.message.contains("by the owner: stop the line"), "{}", e.message);

        let response = engine.handle_request(Request::TaskRun { id: task.id.clone(), reason: Some("try again".into()) }).await;
        assert!(matches!(response, Response::Ok { .. }), "{response:?}");
        let entries = engine.store.entries(&task.id, 50).await.unwrap();
        assert_eq!(data_str(entry_of(&entries, "run_requested"), "reason"), Some("try again"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_agent_asking_is_journaled_as_an_agent_and_a_blank_reason_is_none() {
        let caller = Caller::Agent { scope: "demo".into(), name: "w".into(), role: Role::worker(), run_id: None };
        let asked = Asked::new(&caller, Some("  ".into()));
        assert_eq!(asked.source, "agent");
        assert!(asked.by.contains("w"));
        assert_eq!(asked.reason, None);
        assert_eq!(asked.words(), format!("by {}", asked.by));
    }
}
