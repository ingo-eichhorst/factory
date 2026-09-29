//! Pending tasks for what they are (`#124`): created and not started. Most
//! have no queue behind them and no capacity to wait on -- the daemon only
//! ever starts a scheduled slot that has come due (or a queued retry), and
//! an unscheduled task sits in `pending` until someone runs it. The
//! exception is a task holding a `slot_wait` (`#179`): a declared
//! `max_sessions` limit is already spent, and the daemon itself is holding
//! it for a slot rather than waiting on a schedule or a person. So the one
//! pile the dashboard used to call "Queued" is four different things, and
//! this splits them. Pure: no DOM, no state, the clock passed in.

/// Due and not dispatched -- exactly what Operations' `flow.queue_depth`
/// counts (`factory-core/src/operations.rs`): pending, scheduled, the
/// schedule not paused, its slot already here, and not already claimed by a
/// capacity wait (`#179`) -- that one is `waiting`, not `due`, however late
/// its own slot has grown. Keep the two in step; a dashboard and an
/// Operations page that disagree about the queue is the bug this file
/// exists to fix.
export function isDue(t, now) {
  return (
    t.status === "pending" &&
    !t.slot_wait &&
    !t.after &&
    !!t.schedule &&
    !t.schedule_paused &&
    !!t.next_run_at &&
    Date.parse(t.next_run_at) <= now
  );
}

/// Every pending task, in the one bucket it belongs to:
/// - `waiting`: held on a declared `max_sessions` limit (`#179`) -- the
///   daemon already tried to dispatch it and is waiting for a slot, not for
///   a schedule or a person. Checked first: a scheduled task can be both
///   "its slot has come" and "already waiting", and the wait is the more
///   specific -- and more urgent -- of the two.
/// - `due`: its slot has come and the scheduler will fire it -- the queue.
/// - `later`: scheduled, the slot not here yet, or the schedule paused --
///   or waiting on other tasks (`after`, `#178`): a workflow step whose
///   upstream has not finished. Its trigger is "those finished", not a cron
///   slot, and it fires the same way a slot that has come does. Checked
///   before `manual`: it has no schedule, but something will start it.
/// - `manual`: no schedule, so nothing will ever start it but a person (or
///   an agent) calling `task run`.
export function notStarted(tasks, now) {
  const out = { waiting: [], due: [], later: [], manual: [] };
  for (const t of tasks) {
    if (t.status !== "pending") continue;
    if (t.slot_wait) out.waiting.push(t);
    else if (t.after) out.later.push(t);
    else if (!t.schedule) out.manual.push(t);
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
  if (t.slot_wait) {
    return `Waiting for a slot: ${t.slot_wait.agent} already has \`max_sessions\` in use, since ${t.slot_wait.since}. It starts as soon as one opens.`;
  }
  if (t.after) return waitingOn(t);
  if (!t.schedule) {
    return `Created, not dispatched. Nothing starts this task on its own -- press Run, or \`factory task run ${t.id}\`.`;
  }
  if (t.schedule_paused) {
    return "Not dispatched: its schedule is paused, so nothing fires until it is resumed. Run starts it now.";
  }
  return "Not dispatched until its schedule fires. Run starts it now.";
}

/// "Implement #119 and Review #119" -- the titles a waiting task names.
export function afterTitles(after) {
  const titles = (after?.tasks ?? []).map(task => task.title);
  if (titles.length <= 1) return titles[0] ?? "nothing";
  return `${titles.slice(0, -1).join(", ")} and ${titles[titles.length - 1]}`;
}

/// `#178`: what a board or the modal says about a task waiting on others,
/// where a scheduled one names its next slot -- or "" for any other task.
/// A conditional one says it may never run at all.
export function waitingLabel(t) {
  if (!t?.after) return "";
  return `waiting on ${afterTitles(t.after)}`;
}

function waitingOn(t) {
  const conditional = t.after.conditional ? ` It is conditional: it ${t.after.conditional}, and is closed as not planned if the workflow ends without taking its branch.` : "";
  return `Waiting on ${afterTitles(t.after)}: it starts when ${t.after.tasks.length === 1 ? "that finishes" : "they finish"}.${conditional} Starting it now, ahead of them, needs \`factory task run --ignore-wait ${t.id}\`.`;
}
