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

test("due is what Operations' queue_depth counts: scheduled, not paused, slot already here", () => {
  assert.equal(isDue(due, NOW), true);
  assert.equal(isDue(later, NOW), false, "a slot still to come is not due");
  assert.equal(isDue(paused, NOW), false, "a paused schedule fires nothing");
  assert.equal(isDue(manual, NOW), false, "an unscheduled task is never due -- there is no queue (#124)");
  assert.equal(isDue(running, NOW), false, "only a pending task waits");
});

test("every pending task lands in exactly one bucket, and nothing else lands at all", () => {
  const w = notStarted([manual, due, later, paused, running], NOW);
  assert.deepEqual(w.due.map((t) => t.id), ["d1"]);
  assert.deepEqual(w.later.map((t) => t.id), ["l1", "p1"]);
  assert.deepEqual(w.manual.map((t) => t.id), ["m1"]);
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

test("a task that has been started gets no note", () => {
  assert.equal(notStartedNote(running), null);
  assert.equal(notStartedNote(null), null);
});
