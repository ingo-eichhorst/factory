//! Pending tasks for what they are (`#124`): created and not started. There
//! is no queue behind them and no capacity to wait on -- the daemon only ever
//! starts a scheduled slot that has come due (or a queued retry), and an
//! unscheduled task sits in `pending` until someone runs it. So the one pile
//! the dashboard used to call "Queued" is three different things, and this
//! splits them. Pure: no DOM, no state, the clock passed in.

/// Due and not dispatched -- exactly what Operations' `flow.queue_depth`
/// counts (`factory-core/src/operations.rs`): pending, scheduled, the
/// schedule not paused, and its slot already here. Keep the two in step; a
/// dashboard and an Operations page that disagree about the queue is the
/// bug this file exists to fix.
export function isDue(t, now) {
  return (
    t.status === "pending" &&
    !!t.schedule &&
    !t.schedule_paused &&
    !!t.next_run_at &&
    Date.parse(t.next_run_at) <= now
  );
}

/// Every pending task, in the one bucket it belongs to:
/// - `due`: its slot has come and the scheduler will fire it -- the queue.
/// - `later`: scheduled, the slot not here yet, or the schedule paused.
/// - `manual`: no schedule, so nothing will ever start it but a person (or
///   an agent) calling `task run`.
export function notStarted(tasks, now) {
  const out = { due: [], later: [], manual: [] };
  for (const t of tasks) {
    if (t.status !== "pending") continue;
    if (!t.schedule) out.manual.push(t);
    else if (isDue(t, now)) out.due.push(t);
    else out.later.push(t);
  }
  return out;
}

/// What the task modal says about a task nobody has started, or null for any
/// other task. The point is the first one: a manual task that reads as
/// "waiting its turn" waits forever.
export function notStartedNote(t) {
  if (!t || t.status !== "pending") return null;
  if (!t.schedule) {
    return `Created, not dispatched. Nothing starts this task on its own -- press Run, or \`factory task run ${t.id}\`.`;
  }
  if (t.schedule_paused) {
    return "Not dispatched: its schedule is paused, so nothing fires until it is resumed. Run starts it now.";
  }
  return "Not dispatched until its schedule fires. Run starts it now.";
}
