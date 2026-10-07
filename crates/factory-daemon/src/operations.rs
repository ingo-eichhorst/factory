//! Where the Operations report is served, and the few actions it offers
//! (`#106`, the L4 Line tab and `factory stats`). Like `goals/mod.rs`
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
//!   `schedule_skipped`, `answer` and `run_requested` entries (the owner's
//!   answers count as interventions; an agent's run requests are left out
//!   of them), so nothing walks every task's
//!   whole journal, transcripts and all. A passed-over slot needs a look
//!   for [`MISSED_LOOKBACK_HOURS`]; after that it is history, and the
//!   journal still has it.
//! * **Standing agents** as the store has them; the model judges only a
//!   permanent agent, and only on whether its session is there.
//! Scenario signposts are not a process input. The Dashboard reads L5's
//! fact directly; this report never evaluates or gathers them.
//!
//! ## Capacity, where it is declared (`#179`)
//!
//! A scope's own `max_sessions`, when it declares one, is what
//! [`Flow::sessions_max`] shows -- an agent's own cap is enforced at
//! dispatch (`Engine::capacity_for`) but is not summed into a scope figure
//! here, since two agents' caps do not add into one meaningful ceiling.
//! [`OperationsInput::capacity`] is filled from every scope's own
//! `max_sessions` (`Scope::max_sessions`, root included); a scope that
//! declares none leaves it absent -- still unknown, not a limit of zero.
//! `Flow::sessions_in_use` counts the same way admission does: a run
//! `Dispatching` or holding a session, never a session-less approval hold
//! (`#184`). A task waiting on either cap shows up in `queue_depth` as an
//! ordinary `Stage::Queued` item, aged from `Task.slot_wait.since`.
//!
//! ## Actions
//!
//! Every action the tab offers is a request that already existed, or a
//! narrow one beside it, and each is journaled with who asked and why
//! ([`Asked`]): run again (`task.run`), cancel (`task.cancel`), pause and
//! resume (`task.update`'s `schedule_paused`), skip the next slot
//! (`task.skip_next`, [`Engine::skip_next`]), answer a blocked run
//! (`run.answer`, [`Engine::answer_run`]), and close a task with a reason
//! or reopen it (`task.close`/`task.reopen`, [`Engine::close_task`],
//! `#122`). Nothing here starts, stops or
//! changes anything on its own -- a picture, not a controller (design §8).

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Duration, Utc};
use factory_core::error::{FactoryError, Result};
use factory_core::operations::{
    self, HealthWindow, OperationsInput, OperationsReport, ScopeFilter, SkippedSlots,
};
use factory_core::run::RunStatus;
use factory_core::task::{CloseReason, Task, TaskClosure, TaskEntry, TaskFilter, TaskPatch, TaskStatus};

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
/// report counts as an intervention, when the owner gave it.
pub(crate) const ANSWER_KIND: &str = "answer";

/// The journal kind `task.run` writes, with who asked and the run's
/// `queued_at` -- how the report tells an agent's run from a person's.
pub(crate) const RUN_REQUESTED_KIND: &str = "run_requested";


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

#[path = "operations_report.rs"]
mod report;

impl Engine {



    /// `Request::TaskSkipNext`: pass over a scheduled task's next firing.
    ///
    /// * With no retry queued, the next firing is the schedule's slot, and
    ///   the schedule moves to the first slot after it -- or after now, when
    ///   the slot has already passed undispatched, so skipping a late slot
    ///   never turns into firing an earlier one.
    /// * With a retry queued, the retry is the next firing, and skipping it
    ///   ends the streak exactly as `end_retry_streak` does: the regular
    ///   slot the streak stood in front of (`PendingRetry::resume_at`) comes
    ///   back, or the first slot after now if that has passed too. Skipping
    ///   from the retry's own time instead would lose that slot and let an
    ///   `every` schedule drift by the backoff.
    ///
    /// `slot`, when given, is the firing the caller means to skip -- the
    /// `next_run_at` it was looking at. If the schedule has moved on since
    /// (the scheduler fired it, somebody else skipped it) the skip is
    /// refused rather than landing on a firing nobody chose. Either way
    /// it runs under `schedule_lock`, which the scheduler holds while it
    /// re-reads and fires a due task, so a slot is fired or skipped, never
    /// both.
    pub(crate) async fn skip_next(&self, id: &str, slot: Option<DateTime<Utc>>, asked: &Asked) -> Result<Task> {
        let _slot = self.l4.schedule_lock.lock().await;
        let task = self.require(id).await?;
        let Some(s) = &task.schedule else {
            return Err(FactoryError::BadRequest("only a scheduled task has a next slot to skip".into()));
        };
        if task.schedule_paused {
            return Err(FactoryError::BadRequest(
                "the schedule is paused, so no slot is coming to skip; resume it first".into(),
            ));
        }
        let Some(next_firing) = task.next_run_at else {
            return Err(FactoryError::BadRequest("the task has no next slot to skip".into()));
        };
        if let Some(expected) = slot {
            if expected != next_firing {
                return Err(FactoryError::BadRequest(format!(
                    "the next firing is {} now, not {}; the schedule moved on since it was read -- look again",
                    next_firing.to_rfc3339(),
                    expected.to_rfc3339()
                )));
            }
        }
        let now = Utc::now();
        let next = match &task.pending_retry {
            Some(retry) if retry.resume_at > now => retry.resume_at,
            Some(_) => schedule::next_after(s, now)?,
            None => schedule::next_after(s, next_firing.max(now))?,
        };
        let dropped_retry = task.pending_retry.is_some();
        let updated = self
            .l4.store
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
        let then = if dropped_retry { "resuming the regular schedule" } else { "next firing" };
        self.entry(
            id,
            asked.entry(
                "slot_skipped",
                format!(
                    "{what} at {} skipped {}; {then} at {}",
                    next_firing.to_rfc3339(),
                    asked.words(),
                    next.to_rfc3339()
                ),
                serde_json::json!({ "skipped": next_firing, "next": next, "dropped_retry": dropped_retry }),
            ),
        )
        .await;
        self.shared.bus.publish(factory_core::event::Event::TaskUpdated { task: updated.clone() });
        Ok(updated)
    }

    /// `Request::TaskClose` (`#122`): dispose of a task on purpose, with a
    /// reason. A failure waits for exactly this -- the daemon never closes
    /// one on its own -- so it works on a task with no run at all, a task
    /// blocked by a failure, a pending or a scheduled one.
    ///
    /// Refused while a run is active: that run would go on reporting into a
    /// task somebody closed, so it is cancelled first, on purpose. Refused
    /// on a task already closed, whose reason is changed by reopening it
    /// and closing it again -- two lines in the journal rather than one
    /// quietly rewritten. Refused in intake, which has its own `wontfix`.
    ///
    /// Held under `schedule_lock`, so a scheduled task is closed or fired,
    /// never both. A queued retry goes with the close: nothing fires a
    /// closed task.
    pub(crate) async fn close_task(
        &self,
        id: &str,
        reason: CloseReason,
        duplicate_of: Option<String>,
        asked: &Asked,
    ) -> Result<Task> {
        let _slot = self.l4.schedule_lock.lock().await;
        let task = self.require(id).await?;
        if task.status == TaskStatus::Intake {
            return Err(FactoryError::BadRequest(
                "this task is still in intake: close it there (factory intake decide <id> wontfix)".into(),
            ));
        }
        if let Some(run) = self.l4.store.active_run(id).await? {
            return Err(FactoryError::BadRequest(format!(
                "attempt {} of this task is still {}; cancel it before closing the task",
                run.attempt,
                run.status.as_str()
            )));
        }
        if let Some(already) = task.close_reason() {
            return Err(FactoryError::BadRequest(format!(
                "the task is already closed ({}); reopen it first to close it another way",
                already.label()
            )));
        }
        let duplicate_of = duplicate_of.map(|d| d.trim().to_string()).filter(|d| !d.is_empty());
        if let Some(other) = &duplicate_of {
            if reason != CloseReason::Duplicate {
                return Err(FactoryError::BadRequest(
                    "--duplicate-of goes with the reason duplicate, and nothing else".into(),
                ));
            }
            if other == id {
                return Err(FactoryError::BadRequest("a task cannot duplicate itself".into()));
            }
            self.require(other).await?;
        }

        let closure = TaskClosure {
            reason,
            duplicate_of: duplicate_of.clone(),
            note: asked.reason.clone(),
            by: asked.by.clone(),
            at: Utc::now(),
        };
        let updated = self
            .l4.store
            .update(
                id,
                &TaskPatch {
                    status: Some(reason.status()),
                    closure: Some(closure),
                    clear_after: true,
                    clear_pending_retry: task.pending_retry.is_some(),
                    // A closed task waits for nothing (`#179`) -- this is
                    // also how a task only ever waiting for a slot, never
                    // dispatched, is dropped from the line: there is no run
                    // for `TaskCancel` to act on, but closing needs none.
                    clear_slot_wait: task.slot_wait.is_some(),
                    ..Default::default()
                },
            )
            .await?;
        let how = match &duplicate_of {
            Some(other) => format!("as a duplicate of {other}"),
            None => format!("as {}", reason.label()),
        };
        let was = if task.has_failed() { "its last attempt failed; " } else { "" };
        self.entry(
            id,
            asked.entry(
                "closed",
                format!("{was}closed {how} {}", asked.words()),
                serde_json::json!({
                    "close_reason": reason.as_str(),
                    "duplicate_of": duplicate_of,
                    "was": task.status.as_str(),
                    "fail_kind": task.failure.as_ref().and_then(|f| f.kind).map(|k| k.as_str()),
                }),
            ),
        )
        .await;
        self.shared.bus.publish(factory_core::event::Event::TaskUpdated { task: updated.clone() });
        self.l4_service().sweep_workspaces().await;
        // `#274`: a closed task's sandboxed conversation, if it preserved
        // one, is released the same way its worktree is -- no new sweeper,
        // just the existing per-task release point.
        self.remove_preserved_session(id);
        Ok(updated)
    }

    /// `Request::TaskReopen` (`#122`): a closed task back to `Pending`, its
    /// close record gone. A scheduled one picks its schedule up from now --
    /// the slots that passed while it was closed were not missed, and never
    /// fire as a burst of catch-up runs -- exactly as resuming a paused one
    /// does. Its last run's result and error stay: they are still what
    /// happened last. The failure it may have been closed on goes -- a
    /// pending task carrying one would read as a task waiting on a retry.
    pub(crate) async fn reopen_task(&self, id: &str, asked: &Asked) -> Result<Task> {
        let _slot = self.l4.schedule_lock.lock().await;
        let task = self.require(id).await?;
        let Some(was) = task.close_reason() else {
            return Err(FactoryError::BadRequest(format!(
                "the task is {}, not closed; only a closed task is reopened",
                task.status.as_str()
            )));
        };
        // Intake's `wontfix` is a triage verdict; reopening past it would
        // put an item on the line that was never released (`#119`).
        if task.intake.as_ref().is_some_and(|i| i.stage == factory_core::intake::IntakeStage::Wontfix) {
            return Err(FactoryError::BadRequest(
                "this task was closed in intake as wontfix; intake decides it again, not reopen".into(),
            ));
        }
        let next_run_at = match &task.schedule {
            Some(s) if !task.schedule_paused => Some(schedule::next_after(s, Utc::now())?),
            _ => None,
        };
        let updated = self
            .l4.store
            .update(
                id,
                &TaskPatch {
                    status: Some(TaskStatus::Pending),
                    clear_closure: true,
                    // Somebody looked at it and put it back: a failure it
                    // was closed on is dealt with, and must not make the
                    // pending task read as one mid-retry.
                    clear_failure: true,
                    next_run_at,
                    ..Default::default()
                },
            )
            .await?;
        self.entry(
            id,
            asked.entry(
                "reopened",
                format!("reopened {} (it was closed as {})", asked.words(), was.label()),
                serde_json::json!({ "was": was.as_str() }),
            ),
        )
        .await;
        self.shared.bus.publish(factory_core::event::Event::TaskUpdated { task: updated.clone() });
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
    ///
    /// The answer is journaled the moment the text is in the session, before
    /// enter is pressed: text that reached the agent is on record even if
    /// the keypress then fails, so a person retrying after an error is
    /// looking at a journal that says the first attempt typed something. A
    /// failed keypress is journaled too, and returned as the error.
    ///
    /// There is a race nothing here can close: the agent may unblock itself
    /// between the `Blocked` check and the text arriving, and then the
    /// answer lands in whatever it is doing next. The window is one runtime
    /// call wide; the journal records that an answer was sent either way.
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
        let Some(session) = run.session.as_ref() else {
            return Err(FactoryError::BadRequest(format!("run {run_id} has no session to answer into")));
        };
        let runtime = self.shared.registry.runtime(&session.runtime)?;
        runtime.send_text(session, text).await?;
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
        if let Err(e) = runtime.send_keys(session, &["enter".to_string()]).await {
            self.entry(
                &run.task_id,
                TaskEntry::new(
                    "daemon",
                    "answer_unsent",
                    format!("the answer was typed but enter could not be pressed: {e}"),
                )
                .in_run(&run.id),
            )
            .await;
            return Err(e);
        }
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


#[cfg(test)]
mod tests {
    //! Engine-level Operations reports and actions, on a temporary instance
    //! with a real sqlite store -- not `factory_core::operations` itself
    //! (covered on its own), but this module's gathering and journaling,
    //! each exception kind end to end the way a real request reaches it.
    use std::sync::Arc;

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
        /// Refuse every keypress, as a session that died mid-answer would.
        keys_fail: std::sync::atomic::AtomicBool,
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
            if self.keys_fail.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(FactoryError::Other(anyhow::anyhow!("the pane is gone")));
            }
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

    /// Three scopes -- `demo`, `child` nested under it, and `other` -- on a
    /// store in memory, with
    /// [`TypingRuntime`] as the runtime every run's session belongs to.
    fn test_engine() -> (Arc<Engine>, Arc<TypingRuntime>, PathBuf) {
        test_engine_with(|_| Arc::new(SqliteStore::in_memory().unwrap()))
    }

    /// [`test_engine`] on a store of the caller's making -- a file, when a
    /// test has to reach under the store to backdate a row.
    fn test_engine_with(store: impl FnOnce(&PathBuf) -> Arc<dyn TaskStore>) -> (Arc<Engine>, Arc<TypingRuntime>, PathBuf) {
        let root = std::env::temp_dir().join(format!("factory-operations-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("projects/demo/child")).unwrap();
        std::fs::create_dir_all(root.join("projects/other")).unwrap();
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig { power_assertion: false, ..DaemonConfig::default() },
            scope: None,
            scopes: vec![
                scope("demo-id", "demo", root.join("projects/demo")),
                scope("child-id", "child", root.join("projects/demo/child")),
                scope("other-id", "other", root.join("projects/other")),
            ],
            roles: Default::default(),
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let runtime = Arc::new(TypingRuntime::default());
        let mut registry = Registry::with_builtins();
        registry.add_runtime(runtime.clone(), "test");
        let store: Arc<dyn TaskStore> = store(&root);
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
            .l4.store
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
            .l4.store
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

        let report = engine.operations_report(None, HealthWindow::Week, false).await.unwrap();
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
            .l4.store
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

        let report = engine.operations_report(None, HealthWindow::Week, false).await.unwrap();
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

        let report = engine.operations_report(None, HealthWindow::Week, false).await.unwrap();
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
                .l4.store
                .update_run(
                    &run.id,
                    &RunPatch { status: Some(RunStatus::Done), ended_at: Some(run.started_at), ..Default::default() },
                )
                .await
                .unwrap();
        }
        let open = run_of(&engine, &task.id).await;
        engine
            .l4.store
            .update_run(&open.id, &RunPatch { status: Some(RunStatus::Running), ..Default::default() })
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let report = engine.operations_report(None, HealthWindow::Week, false).await.unwrap();
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
            .l4.store
            .update(&late.id, &TaskPatch { next_run_at: Some(Utc::now() - Duration::hours(1)), ..Default::default() })
            .await
            .unwrap();

        // Fired an hour late on a one-minute schedule: the scheduler writes
        // down the slots between, exactly as a real late tick would.
        let missed = task_in(&engine, "demo", "missed", Some(Schedule::Every { seconds: 60 })).await;
        let missed = engine
            .l4.store
            .update(&missed.id, &TaskPatch { next_run_at: Some(Utc::now() - Duration::hours(1)), ..Default::default() })
            .await
            .unwrap();
        engine.l4_service().advance_schedule(&missed).await.unwrap();

        let report = engine.operations_report(None, HealthWindow::Week, false).await.unwrap();
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
        engine.l4.store.put_agent(&agent("keeper", Lifetime::Permanent, AgentState::Gone)).await.unwrap();
        engine.l4.store.put_agent(&agent("temp", Lifetime::Temporary, AgentState::Gone)).await.unwrap();
        engine.l4.store.put_agent(&agent("fine", Lifetime::Permanent, AgentState::Ready)).await.unwrap();

        let report = engine.operations_report(None, HealthWindow::Week, false).await.unwrap();
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
    async fn operations_never_gathers_signposts_and_dashboard_fact_matches_scenarios() {
        let (engine, _, root) = test_engine();
        std::fs::create_dir_all(root.join(".factory/scenarios")).unwrap();
        std::fs::write(
            root.join(".factory/scenarios/slow-year.yaml"),
            "name: slow-year\ntitle: A slow year\nsignposts:\n  - { metric: throughput_week, below: 999 }\n",
        )
        .unwrap();

        assert!(!engine.l5.signpost_cache.is_populated());
        let report = engine.operations_report(None, HealthWindow::Week, false).await.unwrap();
        assert!(!kinds(&report).contains(&ExceptionKind::TriggeredSignpost));
        assert!(!engine.l5.signpost_cache.is_populated(), "Operations never consulted the signpost provider");
        let fact = engine.signposts_fact(Utc::now(), true).await.unwrap();
        assert_eq!(fact.triggered.len(), 1);
        assert_eq!(fact.triggered[0].scenario, "slow-year");
        // The full Scenarios report says the same thing.
        let full = engine.scenarios_report(None).await.unwrap();
        assert_eq!(full.triggered.len(), 1);

        let scoped = engine.operations_report(Some("demo"), HealthWindow::Week, false).await.unwrap();
        assert!(!kinds(&scoped).contains(&ExceptionKind::TriggeredSignpost));
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_parent_scope_reports_the_work_of_the_scopes_nested_under_it() {
        let (engine, _, root) = test_engine();
        // The parent itself has no work at all; its child does.
        let nested = task_in(&engine, "child", "nested", None).await;
        let there = task_in(&engine, "other", "there", None).await;
        blocked_run(&engine, &nested.id, "nested needs a hand").await;
        blocked_run(&engine, &there.id, "not ours").await;
        engine.entry(&nested.id, TaskEntry::new("owner", ANSWER_KIND, "answered").with_data(serde_json::json!({}))).await;

        let report = engine.operations_report(Some("demo"), HealthWindow::Week, true).await.unwrap();
        assert_eq!(report.scope.as_deref(), Some("demo"));
        let e = report.attention.iter().find(|e| e.kind == ExceptionKind::Blocked).expect("the child's block");
        assert_eq!(e.scope.as_deref(), Some("child"));
        assert_eq!(e.reason, "nested needs a hand", "the block reason is read for a nested scope's run too");
        assert!(report.attention.iter().all(|e| e.scope.as_deref() == Some("child")), "{:?}", report.attention);
        assert_eq!(report.flow.iter().map(|f| f.scope.as_str()).collect::<Vec<_>>(), vec!["child"]);
        assert_eq!(report.health.current.interventions, 1, "the child's answer counts under the parent");
        assert_eq!(report.health.current.days.len(), 7, "the charts' detail, asked for");

        // The child alone does not reach up to its parent's other work.
        let child = engine.operations_report(Some("child"), HealthWindow::Week, false).await.unwrap();
        assert_eq!(child.attention.len(), 1);
        assert!(child.health.current.days.is_empty(), "no detail unless asked");
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_task_that_failed_long_ago_stays_in_the_queue() {
        let (engine, _, root) = test_engine_with(|root| {
            Arc::new(SqliteStore::open(&root.join(".factory/ops-test.sqlite")).unwrap())
        });
        let task = task_in(&engine, "demo", "failed in spring", None).await;
        let run = run_of(&engine, &task.id).await;
        engine
            .l4.store
            .update_run(
                &run.id,
                &RunPatch { status: Some(RunStatus::Failed), ended_at: Some(Utc::now()), error: Some("gave up".into()), ..Default::default() },
            )
            .await
            .unwrap();
        // Blocked on that failure, as a failed run leaves its task (`#122`).
        let failed = factory_core::task::TaskFailure {
            kind: Some(factory_core::run::FailKind::AgentFailed),
            run_id: Some(run.id.clone()),
            attempt: Some(run.attempt),
            at: Utc::now(),
        };
        engine
            .l4.store
            .update(&task.id, &TaskPatch { status: Some(TaskStatus::Blocked), failure: Some(failed), ..Default::default() })
            .await
            .unwrap();
        // Well before any history the report reads.
        let then = (Utc::now() - Duration::days(200)).to_rfc3339();
        let conn = rusqlite::Connection::open(root.join(".factory/ops-test.sqlite")).unwrap();
        conn.execute(
            "UPDATE runs SET started_at = ?1, ended_at = ?1,
               data = json_set(data, '$.started_at', ?1, '$.ended_at', ?1) WHERE id = ?2",
            rusqlite::params![then, run.id],
        )
        .unwrap();
        drop(conn);

        let report = engine.operations_report(None, HealthWindow::Week, false).await.unwrap();
        let e = report.attention.iter().find(|e| e.kind == ExceptionKind::FailedExhausted).expect("still exhausted");
        assert_eq!(e.run_id.as_deref(), Some(run.id.as_str()));
        assert_eq!(report.health.current.finished, 0, "fetched for the queue, not counted as this week's work");
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_cancel_that_names_its_run_never_ends_a_different_one() {
        let (engine, _, root) = test_engine();
        let task = task_in(&engine, "demo", "retried meanwhile", None).await;
        let first = run_of(&engine, &task.id).await;
        engine
            .l4.store
            .update_run(&first.id, &RunPatch { status: Some(RunStatus::Failed), ended_at: Some(Utc::now()), ..Default::default() })
            .await
            .unwrap();
        // A retry started after the person was shown the first attempt.
        let retry = run_of(&engine, &task.id).await;

        let response = engine
            .handle_request(Request::TaskCancel { id: task.id.clone(), reason: None, run: Some(first.id.clone()) })
            .await;
        match response {
            Response::Error { message, .. } => assert!(message.contains("no longer the task's active run"), "{message}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        let still = engine.l4.store.get_run(&retry.id).await.unwrap().unwrap();
        assert!(!still.status.is_terminal(), "the retry is untouched");

        let response = engine
            .handle_request(Request::TaskCancel { id: task.id.clone(), reason: None, run: Some(retry.id.clone()) })
            .await;
        assert!(matches!(response, Response::Ok { .. }), "{response:?}");
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_scope_narrows_every_part_and_an_unknown_one_is_refused() {
        let (engine, _, root) = test_engine();
        let here = task_in(&engine, "demo", "here", None).await;
        let there = task_in(&engine, "other", "there", None).await;
        blocked_run(&engine, &here.id, "here").await;
        blocked_run(&engine, &there.id, "there").await;

        let report = engine.operations_report(Some("demo"), HealthWindow::Month, false).await.unwrap();
        assert_eq!(report.scope.as_deref(), Some("demo"));
        assert!(report.attention.iter().all(|e| e.scope.as_deref() == Some("demo")), "{:?}", report.attention);
        assert_eq!(report.flow.len(), 1);
        assert_eq!(report.health.window, HealthWindow::Month);
        assert_eq!(report.flow[0].sessions_max, None, "no limit is declared anywhere, so none is shown");

        assert!(engine.operations_report(Some("nowhere"), HealthWindow::Week, false).await.is_err());
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

        let skipped = engine.skip_next(&task.id, None, &owner("the host is being moved")).await.unwrap();
        let next = skipped.next_run_at.unwrap();
        assert!(next > slot, "past the skipped slot");
        assert!(next <= slot + Duration::seconds(3600) + Duration::seconds(5), "and no further than the one after it");

        let entries = engine.l4.store.entries(&task.id, 50).await.unwrap();
        let e = entry_of(&entries, "slot_skipped");
        assert_eq!(e.source, "owner");
        assert_eq!(data_str(e, "by"), Some("the owner"));
        assert_eq!(data_str(e, "reason"), Some("the host is being moved"));
        assert!(!entries.iter().any(|e| e.kind == "schedule_skipped"), "a decision is not a missed slot");

        let one_off = task_in(&engine, "demo", "one-off", None).await;
        let err = engine.skip_next(&one_off.id, None, &owner("x")).await.unwrap_err();
        assert!(err.to_string().contains("only a scheduled task"), "{err}");
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn skipping_a_late_slot_never_fires_an_earlier_one() {
        let (engine, _, root) = test_engine();
        let task = task_in(&engine, "demo", "hourly", Some(Schedule::Every { seconds: 3600 })).await;
        engine
            .l4.store
            .update(&task.id, &TaskPatch { next_run_at: Some(Utc::now() - Duration::hours(5)), ..Default::default() })
            .await
            .unwrap();
        let skipped = engine.skip_next(&task.id, None, &owner("stale")).await.unwrap();
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

        let entries = engine.l4.store.run_entries(&run.id, 50).await.unwrap();
        let e = entry_of(&entries, ANSWER_KIND);
        assert_eq!(data_str(e, "by"), Some("the owner"));
        assert_eq!(data_str(e, "reason"), Some("it asked which key"));
        assert!(!e.message.contains("staging"), "the text itself is not journaled: {}", e.message);
        assert_eq!(
            engine.require_run(&run.id).await.unwrap().status,
            RunStatus::Blocked,
            "whether it is unblocked is the agent's to say"
        );

        let report = engine.operations_report(None, HealthWindow::Week, false).await.unwrap();
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
            .l4.store
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
            .handle_request(Request::TaskCancel { id: task.id.clone(), reason: Some("wrong branch".into()), run: None })
            .await;
        assert!(matches!(response, Response::Ok { .. }), "{response:?}");
        let entries = engine.l4.store.run_entries(&run.id, 50).await.unwrap();
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
        let entries = engine.l4.store.entries(&task.id, 50).await.unwrap();
        let e = entry_of(&entries, "schedule_paused");
        assert_eq!(e.source, "owner");
        assert_eq!(data_str(e, "reason"), Some("stop the line"));
        assert!(e.message.contains("by the owner: stop the line"), "{}", e.message);

        let response = engine
            .handle_request(Request::TaskRun { override_wait: false, id: task.id.clone(), reason: Some("try again".into()), continue_run: false })
            .await;
        assert!(matches!(response, Response::Ok { .. }), "{response:?}");
        let entries = engine.l4.store.entries(&task.id, 50).await.unwrap();
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

    // -- review fixes -------------------------------------------------------

    #[tokio::test]
    async fn only_the_owners_answers_and_runs_again_are_interventions() {
        let (engine, _, root) = test_engine();
        let task = task_in(&engine, "demo", "flaky", None).await;
        let first = run_of(&engine, &task.id).await;
        engine.fail_run(&first.id, FailKind::AgentFailed, "exit 1").await;
        // An agent asks for the run again: journaled with its queued_at,
        // exactly as `task.run` writes it.
        let again = run_of(&engine, &task.id).await;
        let agent = Asked::new(
            &Caller::Agent { scope: "demo".into(), name: "foreman".into(), role: Role::worker(), run_id: None },
            Some("tidying up".into()),
        );
        engine
            .entry(
                &task.id,
                agent.entry(RUN_REQUESTED_KIND, "run requested".into(), serde_json::json!({ "queued_at": again.queued_at })),
            )
            .await;
        // And answers a blocked run of another task.
        let other = task_in(&engine, "demo", "asks", None).await;
        let blocked = blocked_run(&engine, &other.id, "?").await;
        engine.entry(&other.id, agent.entry(ANSWER_KIND, "answered".into(), serde_json::json!({})).in_run(&blocked.id)).await;

        let report = engine.operations_report(None, HealthWindow::Week, false).await.unwrap();
        assert_eq!(report.health.current.interventions, 0, "nothing here was the owner's doing");

        // The owner's answer is.
        engine.answer_run(&blocked.id, "yes", &owner("it asked")).await.unwrap();
        let report = engine.operations_report(None, HealthWindow::Week, false).await.unwrap();
        assert_eq!(report.health.current.interventions, 1);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn skipping_a_queued_retry_resumes_the_regular_slot_it_stood_in_front_of() {
        let (engine, _, root) = test_engine();
        let task = task_in(&engine, "demo", "hourly", Some(Schedule::Every { seconds: 3600 })).await;
        let regular = Utc::now() + Duration::minutes(40);
        engine
            .l4.store
            .update(
                &task.id,
                &TaskPatch {
                    next_run_at: Some(Utc::now() + Duration::minutes(5)),
                    pending_retry: Some(factory_core::task::PendingRetry { attempts: 1, resume_at: regular }),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let skipped = engine.skip_next(&task.id, None, &owner("not today")).await.unwrap();
        assert_eq!(skipped.next_run_at, Some(regular), "the regular slot, not one drifted by the backoff");
        assert!(skipped.pending_retry.is_none());
        let entries = engine.l4.store.entries(&task.id, 50).await.unwrap();
        let e = entry_of(&entries, "slot_skipped");
        assert!(e.message.contains(&regular.to_rfc3339()), "{}", e.message);

        // A streak whose regular slot has passed as well resumes from now.
        engine
            .l4.store
            .update(
                &task.id,
                &TaskPatch {
                    pending_retry: Some(factory_core::task::PendingRetry {
                        attempts: 1,
                        resume_at: Utc::now() - Duration::minutes(10),
                    }),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let skipped = engine.skip_next(&task.id, None, &owner("still not")).await.unwrap();
        let next = skipped.next_run_at.unwrap();
        assert!(next > Utc::now() && next <= Utc::now() + Duration::seconds(3600), "{next}");
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_skip_for_a_slot_that_moved_on_is_refused_and_the_scheduler_leaves_a_skipped_slot_alone() {
        let (engine, _, root) = test_engine();
        let task = task_in(&engine, "demo", "hourly", Some(Schedule::Every { seconds: 3600 })).await;
        let seen = task.next_run_at.unwrap();

        let stale = seen - Duration::hours(1);
        let err = engine.skip_next(&task.id, Some(stale), &owner("x")).await.unwrap_err();
        assert!(err.to_string().contains("moved on"), "{err}");
        assert_eq!(engine.require(&task.id).await.unwrap().next_run_at, Some(seen), "nothing moved");

        // The scheduler read the task as due; a skip lands before it fires.
        let as_the_scheduler_saw_it = engine.require(&task.id).await.unwrap();
        engine.skip_next(&task.id, Some(seen), &owner("x")).await.unwrap();
        assert!(engine.l4_service().still_due(&as_the_scheduler_saw_it).await.is_none(), "the skipped slot is not fired");
        let fresh = engine.require(&task.id).await.unwrap();
        assert!(engine.l4_service().still_due(&fresh).await.is_some());
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn an_answer_is_on_record_once_typed_even_when_enter_fails() {
        let (engine, runtime, root) = test_engine();
        let task = task_in(&engine, "demo", "t", None).await;
        let run = blocked_run(&engine, &task.id, "?").await;
        runtime.keys_fail.store(true, std::sync::atomic::Ordering::SeqCst);

        assert!(engine.answer_run(&run.id, "yes", &owner("it asked")).await.is_err());
        let kinds: Vec<String> = engine.l4.store.run_entries(&run.id, 50).await.unwrap().into_iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&ANSWER_KIND.to_string()), "{kinds:?}");
        assert!(kinds.contains(&"answer_unsent".to_string()), "{kinds:?}");
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn signposts_are_reused_until_a_scenario_file_changes() {
        let (engine, _, root) = test_engine();
        let dir = root.join(".factory/scenarios");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("slow-year.yaml");
        std::fs::write(&file, "name: slow-year\ntitle: A slow year\nsignposts:\n  - { metric: throughput_week, below: 999 }\n").unwrap();

        let now = Utc::now();
        assert_eq!(engine.triggered_signposts_cached(now).await.unwrap().len(), 1);
        assert!(engine.l5.signpost_cache.is_populated(), "kept by L5");
        assert_eq!(engine.triggered_signposts_cached(now).await.unwrap().len(), 1, "served from the cache");

        std::fs::remove_file(&file).unwrap();
        assert!(engine.triggered_signposts_cached(now).await.unwrap().is_empty(), "a removed file is not answered from the cache");
        std::fs::remove_dir_all(root).ok();
    }
}
