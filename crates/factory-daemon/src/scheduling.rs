//! Scheduling and admission (#193 phase 6, S9a part 1): which tasks are due, which are waiting on
//! a dependency or a slot, when a schedule fires next, and how much capacity an agent has left.
//! L4's decisions over its own tasks and runs; the scheduler loop (`scheduler.rs`) only drives them
//! on a timer and spawns the dispatches. Everything here reads and writes L4's store and nothing else.
use crate::admission::{capacity, run_uses_a_slot, Capacity};
use crate::engine::{dispatchable_from, skip_reason_of, CapacityEvent, Due, SkipReason};
use crate::schedule;
use crate::l4_service::L4Service;
use chrono::Utc;
use factory_core::event::Event;
use factory_core::error::Result;
use factory_core::run::{FailKind, Run};
use factory_core::task::{Task, TaskEntry, TaskFailure, TaskFilter, TaskPatch, TaskStatus};

impl L4Service<'_> {
    pub async fn due_now(&self) -> Result<Vec<Task>> {
        // The sqlite store already passes over a paused schedule; an
        // out-of-process store may predate the field and not know to, so
        // the rule is held here as well rather than trusted to every
        // adapter.
        let mut due = self.state.store.due(Utc::now()).await?;
        due.retain(|t| !t.schedule_paused && t.fires());
        Ok(due)
    }
    /// Move a scheduled task's next firing forward so it is not picked up twice
    /// while it runs.
    ///
    /// The next firing is computed from now, not from the slot that is
    /// firing, so a task that was still running through later slots -- or a
    /// daemon that was down through them -- fires once, not once per slot.
    /// Those passed-over slots are written down here, as one
    /// `schedule_skipped` entry, because this is the only place that knows
    /// they existed: nothing else would ever say the 03:00 run did not
    /// happen.
    pub async fn advance_schedule(&self, task: &Task) -> Result<()> {
        let Some(s) = &task.schedule else {
            return Ok(());
        };
        let now = Utc::now();
        if let Some(slot) = task.next_run_at {
            let tick = chrono::Duration::seconds(self.wiring.snapshot().config.daemon.tick_seconds.max(1) as i64);
            if let Some(skipped) = schedule::skipped_beyond_tick(s, slot, now, tick) {
                let reason = self.skip_reason(task, skipped.first).await;
                self.entry(
                    &task.id,
                    TaskEntry::new(
                        "scheduler",
                        "schedule_skipped",
                        format!(
                            "{} slot{} between {} and {} passed without firing ({}); \
                             firing once for {} instead of catching up",
                            skipped.count,
                            if skipped.count == 1 { "" } else { "s" },
                            skipped.first.to_rfc3339(),
                            skipped.last.to_rfc3339(),
                            reason.describe(),
                            slot.to_rfc3339(),
                        ),
                    )
                    .with_data(serde_json::json!({
                        "count": skipped.count,
                        "first": skipped.first,
                        "last": skipped.last,
                        "capped": skipped.capped,
                        "fired_slot": slot,
                        "reason": reason.as_str(),
                    })),
                )
                .await;
            }
        }
        let next = schedule::next_after(s, now)?;
        let updated = self
            .state.store
            .update(
                &task.id,
                &TaskPatch {
                    next_run_at: Some(next),
                    ..Default::default()
                },
            )
            .await?;
        self.wiring.bus().publish(Event::TaskUpdated { task: updated });
        Ok(())
    }
    /// The `Due` for a scheduled firing (`scheduled: true`, `at` is the slot)
    /// or a queued retry (`at` is when its backoff ran out): due at `at`,
    /// queued from the moment it could first have been dispatched -- see
    /// `dispatchable_from`. Read before `advance_schedule` or
    /// `resume_from_retry` moves `next_run_at` on, and before the new run
    /// exists, so the newest run is the previous one.
    pub async fn due_for(&self, task: &Task, at: chrono::DateTime<Utc>, scheduled: bool) -> Due {
        let previous = self.state.store.runs(&task.id, 1).await.ok().and_then(|r| r.into_iter().next());
        let mut due = if scheduled { Due::slot(at) } else { Due::retry(at) };
        due.queued_at = dispatchable_from(at, self.wiring.booted_at(), previous.as_ref());
        due
    }
    /// Why the slots from `first` on passed without firing: the task's own
    /// previous run was still going at `first`, or it was not -- in which
    /// case the daemon was not there to fire it (asleep, stopped, or its
    /// tick running late). Read off the newest run's own times, the only
    /// record either way.
    async fn skip_reason(&self, task: &Task, first: chrono::DateTime<Utc>) -> SkipReason {
        let newest = self.state.store.runs(&task.id, 1).await.ok().and_then(|r| r.into_iter().next());
        skip_reason_of(newest.as_ref(), first)
    }
    /// Move a queued retry's task back onto the regular slot its streak is
    /// standing in front of, before dispatching the retry attempt itself --
    /// the retry's own version of what `advance_schedule` does for a regular
    /// firing above, and for the same reason: a dispatch slower than the
    /// backoff must not pick the task up a second time.
    ///
    /// Deliberately does not recompute the schedule the way `advance_schedule`
    /// does: `resume_at` was captured once, in `queue_or_end_retry`, when this
    /// streak began, and must survive however many retries happen before it
    /// ends. Recomputing from `now` here instead would be redundant at best
    /// for a `Cron` schedule (grid-aligned, so it would usually land on the
    /// same slot anyway) and actively wrong for an `Every` schedule, which
    /// has no grid at all -- each recompute would push the regular firing
    /// further out, which is exactly the displacement the scheduler's own
    /// comment on `advance_schedule` running before dispatch warns against.
    ///
    /// Dispatches this retry unconditionally, even if the task's policy was
    /// edited to `retry: none` after this streak began: a retry already
    /// queued and due is one the daemon committed to when it queued it, and
    /// pulling a run back out from under a dispatch already under way would
    /// be its own kind of surprise. `queue_or_end_retry` is where the new
    /// policy actually takes effect -- honoured on this attempt's outcome,
    /// not retroactively on the attempt itself.
    pub async fn resume_from_retry(&self, task: &Task) -> Result<()> {
        let Some(retry) = &task.pending_retry else {
            return Ok(());
        };
        let updated = self
            .state.store
            .update(
                &task.id,
                &TaskPatch {
                    next_run_at: Some(retry.resume_at),
                    ..Default::default()
                },
            )
            .await?;
        self.wiring.bus().publish(Event::TaskUpdated { task: updated });
        Ok(())
    }
    /// Every task still stored as `failed` -- written before `#122`, when a
    /// failed run closed its task -- moved to `Blocked` on that failure, so
    /// the board shows it where a person has to act instead of in a column
    /// it no longer has. The failure is read off the newest run; a task
    /// that never got a run (dispatch refused) has none to read, and is
    /// blocked on a dispatch failure, which is the only way that happened.
    ///
    /// Blocked, not closed as not planned: the rule this exists for is that
    /// a failure is an open item until a person disposes of it, and a bulk
    /// close would be the daemon disposing of every one at once. A
    /// scheduled one picks its schedule up from now -- its slots stopped
    /// firing the moment it failed, and must not come back as a burst.
    ///
    /// Idempotent and cheap: once a row is moved, nothing writes `failed`
    /// again, so later starts find none. Returns how many were moved.
    pub async fn migrate_failed_tasks(&self) -> usize {
        let filter = TaskFilter { status: Some(TaskStatus::Failed), ..Default::default() };
        let Ok(tasks) = self.state.store.list(&filter).await else {
            return 0;
        };
        let mut moved = 0;
        for task in tasks {
            let newest = self.state.store.runs(&task.id, 1).await.ok().and_then(|r| r.into_iter().next());
            let failure = match &newest {
                Some(run) => TaskFailure {
                    kind: run.fail_kind,
                    run_id: Some(run.id.clone()),
                    attempt: Some(run.attempt),
                    at: run.ended_at.unwrap_or(task.updated_at),
                },
                None => TaskFailure {
                    kind: Some(FailKind::DispatchFailed),
                    run_id: None,
                    attempt: None,
                    at: task.updated_at,
                },
            };
            let next_run_at = match &task.schedule {
                Some(s) if !task.schedule_paused => schedule::next_after(s, Utc::now()).ok(),
                _ => None,
            };
            let patch = TaskPatch {
                status: Some(TaskStatus::Blocked),
                failure: Some(failure),
                next_run_at,
                ..Default::default()
            };
            match self.state.store.update(&task.id, &patch).await {
                Ok(updated) => {
                    moved += 1;
                    self.entry(
                        &task.id,
                        TaskEntry::new(
                            "daemon",
                            "migrated",
                            "stored as failed before failures stopped closing tasks (#122): now blocked on that \
                             failure until someone runs it again or closes it with a reason",
                        ),
                    )
                    .await;
                    self.wiring.bus().publish(Event::TaskUpdated { task: updated });
                }
                Err(e) => tracing::warn!(task = %task.id, error = %e, "could not move a failed task to blocked"),
            }
        }
        if moved > 0 {
            tracing::info!(moved, "moved tasks stored as failed to blocked (#122)");
        }
        moved
    }
    /// Human-readable dependencies that are not successfully complete.
    /// Missing ids block too: silently treating a deleted prerequisite as
    /// success would run work with an input it never received.
    pub(crate) async fn dependency_blockers(&self, task: &Task) -> Result<Vec<String>> {
        let mut blockers = Vec::new();
        if task.workflow_origin.is_some() && task.after.is_some() {
            return Ok(vec![self.waiting_description(task).await?]);
        }
        // The graph also owns decomposition merge/gate ordering. Its
        // explicit release (or a journaled override) consumes that wait.
        if task.workflow_origin.is_some() { return Ok(blockers); }
        for id in task.depends_on.iter().chain(task.after.iter().flatten()) {
            match self.state.store.get(id).await? {
                Some(parent) if parent.status == TaskStatus::Done => {}
                Some(parent) => blockers.push(format!(
                    "{} ({}, {})",
                    parent.title,
                    parent.id,
                    parent.status.as_str()
                )),
                None => blockers.push(format!("{id} (missing)")),
            }
        }
        Ok(blockers)
    }
    /// Pending decomposition tasks which became runnable since the last
    /// scheduler tick, including never-started roots recovered after a
    /// restart. `runs == 0` is deliberate: plan release starts a task once;
    /// retrying an attempt that failed remains an explicit act.
    pub(crate) async fn dependency_ready_tasks(&self) -> Result<Vec<Task>> {
        let tasks = self.state.store.list(&TaskFilter::default()).await?;
        let mut ready = Vec::new();
        for task in tasks.into_iter().filter(|task| {
            task.status == TaskStatus::Pending
                && task.runs == 0
                && task.slot_wait.is_none()
                && (task.after.is_some() || (task.parent_task_id.is_some() && task.decomposition_part.is_some()))
        }) {
            if task.workflow_origin.is_some() { continue; }
            if self.dependency_blockers(&task).await?.is_empty() {
                ready.push(task);
            }
        }
        Ok(ready)
    }
    /// Every `Pending` task with a `slot_wait`, oldest wait first -- FIFO,
    /// exactly the order `queued_at` gives it.
    pub(crate) async fn waiting_tasks(&self) -> Vec<Task> {
        let mut tasks: Vec<Task> = self
            .state.store
            .list(&TaskFilter { status: Some(TaskStatus::Pending), ..Default::default() })
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|t| t.slot_wait.is_some())
            .collect();
        tasks.sort_by_key(|t| t.slot_wait.as_ref().unwrap().queued_at);
        tasks
    }
    /// A run just stopped using a slot: send `(scope, agent)` to the capacity
    /// worker so whatever is waiting on it wakes right away. `&self`
    /// deliberately -- `finish_run`, `report` and `cancel_task_run` are not
    /// `Arc<Self>`, and a channel send needs no more than that, the same
    /// reason `enqueue_verification` gets away with it.
    pub(crate) fn enqueue_capacity_release(&self, scope: &str, agent: &str) {
        let _ = self.state.capacity_release_tx.send(CapacityEvent::Release(scope.to_string(), agent.to_string()));
    }
    /// The scheduler tick's own sweep, queued rather than run inline: a tick
    /// that blocked on one slow dispatch would delay every due task it fires
    /// afterward, the ack/run-timeout checks, and `supervise_agents` behind
    /// it -- exactly why the due-task loop already spawns each dispatch
    /// instead of awaiting it in place, and `recheck_harnesses` backgrounds
    /// itself. Going through the same channel `Release` does also means a
    /// sweep and a release can never run at once.
    pub(crate) fn enqueue_capacity_sweep(&self) {
        let _ = self.state.capacity_release_tx.send(CapacityEvent::Sweep);
    }
    pub async fn active_runs(&self) -> Result<Vec<Run>> {
        self.state.store.active_runs().await
    }
    /// For the scheduler, under `schedule_lock`: `seen` as the store has it
    /// now, if it is still due exactly as `due_now` found it -- the same
    /// next firing, the same retry, not paused. `None` when anything moved
    /// in between (a skip, a pause, an edit): that is somebody's decision,
    /// and the next tick reads it afresh.
    pub(crate) async fn still_due(&self, seen: &Task) -> Option<Task> {
        let now = self.state.store.get(&seen.id).await.ok().flatten()?;
        let same = now.next_run_at == seen.next_run_at
            && now.pending_retry == seen.pending_retry
            && !now.schedule_paused
            && now.schedule.is_some()
            // Closed in between (`close_task` holds the same lock) --
            // nothing fires a closed task, or dispatch would undo the close.
            && now.fires();
        same.then_some(now)
    }
    pub(crate) async fn waiting_description(&self, task: &Task) -> Result<String> {
        let mut names = Vec::new();
        for id in task.after.iter().flatten() {
            names.push(match self.state.store.get(id).await? {
                Some(parent) => format!("{} ({id})", parent.title),
                None => format!("{id} (missing)"),
            });
        }
        let mut description = if names.is_empty() {
            "waiting for workflow release".into()
        } else {
            format!("waiting on {}", names.join(", "))
        };
        if let Some(condition) = &task.after_condition {
            description.push_str(&format!("; {condition}"));
        }
        Ok(description)
    }

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
        let scope_max = self.wiring.snapshot().scope(scope).ok().and_then(|s| s.max_sessions);
        let runs = self.state.store.active_runs().await?;
        let mut counted = Vec::with_capacity(runs.len());
        for run in &runs {
            if !run_uses_a_slot(run.status, run.session.is_some()) {
                continue;
            }
            if let Ok(Some(task)) = self.state.store.get(&run.task_id).await {
                counted.push((task.scope, run.agent.clone()));
            }
        }
        // L3's fact, read as the level above it: whether a live permanent agent of
        // this name occupies a slot of its own.
        let permanent_agent_live = self.wiring.facts()
            .get::<factory_kernel::StandingAgentLiveFact>(&(scope.to_string(), agent.to_string()))
            .await
            .map(|fact| fact.live)
            .unwrap_or(false);
        Ok(capacity(scope, agent, agent_max, scope_max, counted, permanent_agent_live))
    }
}
