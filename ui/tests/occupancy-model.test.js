import test from "node:test";
import assert from "node:assert/strict";

import {
  rowLanes, rowStyle, laneStyle, concurrency, blockedStretches, blockedNote, waitingShare,
} from "../js/occupancy-model.js";

test("a row that never overlapped carries no lane geometry at all", () => {
  // Exactly the markup from before #120: no style on the row, none on a block.
  assert.equal(rowLanes({ lanes: 1 }), 1);
  assert.equal(rowStyle({ lanes: 1 }), "");
  assert.equal(laneStyle({ lane: 0 }), "");
  assert.equal(concurrency({ agent: "sh", live: 1 }), null);
  assert.equal(concurrency({ agent: "sh", live: 0 }), null);
});

test("a payload from a daemon without lanes is one lane", () => {
  assert.equal(rowLanes({}), 1);
  assert.equal(rowStyle({}), "");
  assert.equal(laneStyle({}), "");
  assert.equal(concurrency({ agent: "sh" }), null);
});

test("three concurrent runs stack into three lanes", () => {
  const row = { agent: "builder", lanes: 3, live: 3 };
  assert.equal(rowLanes(row), 3);
  assert.equal(rowStyle(row), ` style="--lanes:3"`);
  assert.deepEqual([0, 1, 2].map((lane) => laneStyle({ lane })), ["", "--lane:1;", "--lane:2;"]);
  assert.deepEqual(concurrency(row), { tag: "×3", title: "3 runs of builder are live at once" });
});

test("garbage in a lane field never becomes a style", () => {
  assert.equal(rowStyle({ lanes: "3;color:red" }), "");
  assert.equal(laneStyle({ lane: "1;color:red" }), "");
  assert.equal(laneStyle({ lane: -1 }), "");
});

// -- blocked stretches (#121) -------------------------------------------------

const T0 = Date.parse("2026-09-25T16:20:39Z");
const iso = (secs) => new Date(T0 + secs * 1000).toISOString();
const span = (secs) => `${Math.round(secs)}s`;

test("a run that never blocked has no stretches and no note", () => {
  const run = { from: iso(0), to: iso(60), status: "done" };
  assert.deepEqual(blockedStretches(run, T0 + 90e3), []);
  assert.deepEqual(blockedStretches({ ...run, segments: [] }, T0 + 90e3), []);
  assert.equal(blockedNote([], span), "");
});

test("a finished run keeps the wait it spent blocked", () => {
  // #121's own run: blocked 16:20:50, done 16:25:18.
  const run = { from: iso(0), to: iso(279), status: "done", segments: [{ status: "blocked", from: iso(11), to: iso(279) }] };
  const waits = blockedStretches(run, T0 + 999e3);
  assert.deepEqual(waits, [{ from: T0 + 11e3, to: T0 + 279e3, seconds: 268 }]);
  assert.equal(blockedNote(waits, span), "blocked 268s waiting for a human");
});

test("an open wait runs to now, and never past the run", () => {
  const open = { from: iso(0), status: "blocked", segments: [{ status: "blocked", from: iso(5) }] };
  assert.deepEqual(blockedStretches(open, T0 + 30e3), [{ from: T0 + 5e3, to: T0 + 30e3, seconds: 25 }]);
  const ended = { from: iso(0), to: iso(20), status: "cancelled", segments: [{ status: "blocked", from: iso(5), to: iso(40) }] };
  assert.equal(blockedStretches(ended, T0 + 60e3)[0].to, T0 + 20e3);
});

test("several waits are named together", () => {
  const run = {
    from: iso(0), to: iso(100), status: "done",
    segments: [{ status: "blocked", from: iso(10), to: iso(40) }, { status: "blocked", from: iso(60), to: iso(70) }],
  };
  const waits = blockedStretches(run, T0 + 200e3);
  assert.equal(waits.length, 2);
  assert.equal(blockedNote(waits, span), "blocked 2× for 40s waiting for a human");
});

test("garbage segments draw nothing", () => {
  const run = { from: iso(0), to: iso(10), segments: [null, { status: "running", from: iso(1), to: iso(2) }, { status: "blocked", from: iso(5), to: iso(5) }] };
  assert.deepEqual(blockedStretches(run, T0 + 20e3), []);
  assert.deepEqual(blockedStretches(null, T0), []);
});

test("the waiting share is of busy time, and absent without a wait", () => {
  assert.equal(waitingShare({ busy_seconds: 279 }), null);
  assert.equal(waitingShare({ busy_seconds: 279, blocked_seconds: 0 }), null);
  assert.equal(waitingShare({ busy_seconds: 0, blocked_seconds: 5 }), null);
  assert.deepEqual(waitingShare({ busy_seconds: 279, blocked_seconds: 268 }), {
    pct: 96, title: "96% of the busy time was spent blocked, waiting for a human",
  });
});
