import test from "node:test";
import assert from "node:assert/strict";

import { isDue, notStarted, notStartedNote } from "../js/pending-model.js";

const NOW = Date.parse("2026-09-25T18:00:00Z");
const every = { every: { seconds: 300 } };

const manual = { id: "m1", status: "pending", schedule: null, next_run_at: null };
const due = { id: "d1", status: "pending", schedule: every, next_run_at: "2026-09-25T17:55:00Z" };
const later = { id: "l1", status: "pending", schedule: every, next_run_at: "2026-09-25T18:05:00Z" };
const paused = { id: "p1", status: "pending", schedule: every, schedule_paused: true, next_run_at: "2026-09-25T17:00:00Z" };
const running = { id: "r1", status: "running", schedule: every, next_run_at: "2026-09-25T17:00:00Z" };
const slotWait = { agent: "codex", scope: "demo", trigger: "manual", queued_at: "2026-09-25T17:50:00Z", since: "2026-09-25T17:50:00Z" };
const waiting = { id: "w1", status: "pending", schedule: null, next_run_at: null, slot_wait: slotWait };
const waitingAndDue = {
  id: "w2",
  status: "pending",
  schedule: every,
  next_run_at: "2026-09-25T17:55:00Z",
  slot_wait: slotWait,
};

test("due is what Operations' queue_depth counts: scheduled, not paused, slot already here", () => {
  assert.equal(isDue(due, NOW), true);
  assert.equal(isDue(later, NOW), false, "a slot still to come is not due");
  assert.equal(isDue(paused, NOW), false, "a paused schedule fires nothing");
  assert.equal(isDue(manual, NOW), false, "an unscheduled task is never due -- there is no queue (#124)");
  assert.equal(isDue(running, NOW), false, "only a pending task waits");
  assert.equal(isDue(waitingAndDue, NOW), false, "already claimed by a capacity wait (#179), not due again");
});

test("every pending task lands in exactly one bucket, and nothing else lands at all", () => {
  const w = notStarted([manual, due, later, paused, running, waiting], NOW);
  assert.deepEqual(w.due.map((t) => t.id), ["d1"]);
  assert.deepEqual(w.later.map((t) => t.id), ["l1", "p1"]);
  assert.deepEqual(w.manual.map((t) => t.id), ["m1"]);
  assert.deepEqual(w.waiting.map((t) => t.id), ["w1"]);
});

test("a task waiting for a slot is never double-counted with its own overdue schedule (#179)", () => {
  const w = notStarted([waitingAndDue], NOW);
  assert.deepEqual(w.waiting.map((t) => t.id), ["w2"]);
  assert.deepEqual(w.due, []);
});

test("a manual task says it was not dispatched and how to run it", () => {
  const note = notStartedNote(manual);
  assert.match(note, /^Created, not dispatched/);
  assert.match(note, /Nothing starts this task on its own/);
  assert.match(note, /`factory task run m1`/);
});

test("a scheduled task says its schedule fires it, or that a paused one will not", () => {
  assert.match(notStartedNote(later), /until its schedule fires/);
  assert.match(notStartedNote(paused), /paused/);
});

test("a task waiting for a slot says which agent and since when (#179)", () => {
  const note = notStartedNote(waiting);
  assert.match(note, /^Waiting for a slot/);
  assert.match(note, /codex/);
  assert.match(note, /max_sessions/);
});

test("a task that has been started gets no note", () => {
  assert.equal(notStartedNote(running), null);
  assert.equal(notStartedNote(null), null);
});

test("#178: a task waiting on another is scheduled later, never due or manual", async () => {
  const { notStarted, isDue, notStartedNote, waitingLabel, afterTitles } = await import("../js/pending-model.js");
  const now = Date.parse("2026-09-29T12:00:00Z");
  const waiting = {
    id: "b", status: "pending",
    after: { tasks: [{ id: "a", title: "Implement #119" }] },
  };
  const conditional = {
    id: "c", status: "pending",
    after: { tasks: [{ id: "a", title: "Implement #119" }, { id: "r", title: "Review #119" }], conditional: "runs only if Review #119 routes the work this way" },
  };
  const manual = { id: "m", status: "pending" };
  assert.equal(isDue(waiting, now), false);
  const buckets = notStarted([waiting, conditional, manual], now);
  assert.deepEqual(buckets.later.map(t => t.id), ["b", "c"]);
  assert.deepEqual(buckets.manual.map(t => t.id), ["m"]);
  assert.deepEqual(buckets.due, []);
  assert.equal(waitingLabel(waiting), "waiting on Implement #119");
  assert.equal(afterTitles(conditional.after), "Implement #119 and Review #119");
  assert.equal(waitingLabel(manual), "");
  assert.match(notStartedNote(waiting), /^Waiting on Implement #119: it starts when that finishes\. Starting it now/);
  assert.match(notStartedNote(conditional), /conditional: it runs only if Review #119 routes the work this way/);
  assert.match(notStartedNote(waiting), /--ignore-wait b/);
});
