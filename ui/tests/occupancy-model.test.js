import test from "node:test";
import assert from "node:assert/strict";

import {
  rowLanes, rowStyle, laneStyle, concurrency, blockedStretches, blockedNote, waitingShare,
  MIN_SPAN, MAX_SPAN, tickStep, tickTimes, liveView, resolveWindow, settle, isPreset, zoomAround, pan,
  panByPixels, wheelFactor, fractionAt, buttonAnchor, windowQuery, overlaps,
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

const MIN = 60000;
const HOUR = 60 * MIN;
const NOW = Date.UTC(2026, 8, 25, 12, 0, 0);

test("the preset is the daemon's default window: minutes back, a quarter ahead", () => {
  const view = liveView(720);
  assert.deepEqual(resolveWindow(view, NOW), { from: NOW - 12 * HOUR, to: NOW + 3 * HOUR });
  assert.equal(isPreset(view, 720), true);
  assert.equal(isPreset(view, 60), false, "a different preset is somewhere to go back to");
  assert.deepEqual(resolveWindow(liveView(1), NOW), { from: NOW - MIN, to: NOW + MIN }, "never nothing ahead");
});

test("a live view follows the clock; a fixed one stays where it was put", () => {
  const live = liveView(60);
  const later = NOW + 10 * MIN;
  assert.deepEqual(resolveWindow(live, later), { from: later - HOUR, to: later + 15 * MIN });
  const fixed = { live: false, from: NOW - 5 * HOUR, to: NOW - 4 * HOUR };
  assert.deepEqual(resolveWindow(fixed, later), { from: fixed.from, to: fixed.to });
});

test("a window with now in it settles live, one without settles fixed", () => {
  const across = { from: NOW - 2 * HOUR, to: NOW + HOUR };
  assert.deepEqual(settle(across, NOW), { live: true, before: 2 * HOUR, after: HOUR });
  const past = { from: NOW - 3 * HOUR, to: NOW - HOUR };
  assert.deepEqual(settle(past, NOW), { live: false, ...past });
  const future = { from: NOW + HOUR, to: NOW + 3 * HOUR };
  assert.deepEqual(settle(future, NOW), { live: false, ...future });
  // Settling the preset changes nothing, so a poll never nudges it.
  const preset = liveView(720);
  assert.deepEqual(settle(resolveWindow(preset, NOW), NOW), preset);
  // A fixed future window the clock walks into starts following.
  assert.equal(settle(resolveWindow({ live: false, ...future }, NOW + 2 * HOUR), NOW + 2 * HOUR).live, true);
});

test("zooming keeps the point under the cursor where it was", () => {
  const win = { from: 0, to: 10 * HOUR };
  for (const fraction of [0, 0.25, 0.5, 0.9, 1]) {
    for (const factor of [0.5, 2, 1.3]) {
      const anchor = win.from + fraction * (win.to - win.from);
      const z = zoomAround(win, factor, fraction);
      assert.equal(z.to - z.from, (win.to - win.from) * factor);
      assert.ok(Math.abs(z.from + fraction * (z.to - z.from) - anchor) < 1e-6, `${fraction} × ${factor}`);
    }
  }
});

test("zoom is clamped to the daemon's bounds, on both ends", () => {
  const win = { from: NOW - HOUR, to: NOW };
  const tight = zoomAround(win, 0.001, 0.5);
  assert.equal(tight.to - tight.from, MIN_SPAN);
  assert.equal((tight.from + tight.to) / 2, NOW - HOUR / 2, "clamped around the same point");
  const wide = zoomAround(win, 1e6, 1);
  assert.equal(wide.to - wide.from, MAX_SPAN);
  assert.equal(wide.to, NOW, "the right edge is the anchor, so it stays");
  // An anchor off the ends is held to them rather than flinging the window.
  assert.deepEqual(zoomAround(win, 2, 7), zoomAround(win, 2, 1));
  assert.deepEqual(zoomAround(win, 2, NaN), zoomAround(win, 2, 0.5));
});

test("a pan moves both edges and never the width", () => {
  const win = { from: NOW - HOUR, to: NOW };
  assert.deepEqual(pan(win, -30 * MIN), { from: NOW - 90 * MIN, to: NOW - 30 * MIN });
  // Dragging right by a quarter of the track shows a quarter of the window earlier.
  assert.deepEqual(panByPixels(win, 250, 1000), { from: NOW - 75 * MIN, to: NOW - 15 * MIN });
  assert.deepEqual(panByPixels(win, -500, 1000), { from: NOW - 30 * MIN, to: NOW + 30 * MIN });
  assert.equal(panByPixels(win, 100, 0), win, "a track with no width moves nothing");
});

test("a live window dragged off now becomes fixed, and zoomed around now stays live", () => {
  const live = liveView(60);
  const dragged = settle(panByPixels(resolveWindow(live, NOW), 1000, 800), NOW);
  assert.equal(dragged.live, false, "the whole hour and a quarter behind: now is off the right edge");
  const nudged = settle(panByPixels(resolveWindow(live, NOW), 100, 1000), NOW);
  assert.equal(nudged.live, true, "a small drag still has now in it, so it keeps following");
  assert.equal(nudged.before + nudged.after, 75 * MIN);
  const win = resolveWindow(live, NOW);
  const zoomed = settle(zoomAround(win, 0.5, buttonAnchor(win, NOW)), NOW);
  assert.equal(zoomed.live, true);
  assert.equal(zoomed.before, 30 * MIN);
  assert.equal(buttonAnchor({ from: NOW - 2 * HOUR, to: NOW - HOUR }, NOW), 0.5, "off-window: the middle");
});

test("a wheel zooms by its delta, a pinch more strongly, and one event only so far", () => {
  assert.ok(wheelFactor(100) > 1, "down is out");
  assert.ok(wheelFactor(-100) < 1, "up is in");
  assert.equal(wheelFactor(0), 1);
  assert.ok(Math.abs(wheelFactor(100) * wheelFactor(-100) - 1) < 1e-9, "in and out again is where it started");
  assert.ok(wheelFactor(10, 0, true) > wheelFactor(10, 0, false));
  assert.equal(wheelFactor(3, 1), wheelFactor(48, 0), "lines are sixteen pixels");
  assert.equal(wheelFactor(1e5), 2);
  assert.equal(wheelFactor(-1e5), 0.5);
});

test("the cursor's fraction is clamped to the track", () => {
  assert.equal(fractionAt(150, 100, 200), 0.25);
  assert.equal(fractionAt(50, 100, 200), 0);
  assert.equal(fractionAt(400, 100, 200), 1);
  assert.equal(fractionAt(150, 100, 0), 0.5);
});

test("the query sends both edges in UTC, with nothing a query string would mangle", () => {
  const q = windowQuery({ from: NOW - HOUR, to: NOW + 0.4 });
  assert.equal(q, "from=2026-09-25T11%3A00%3A00.000Z&to=2026-09-25T12%3A00%3A00.000Z");
  assert.ok(!q.includes("+"));
});

test("ticks get finer as the window narrows and coarser as it widens", () => {
  assert.equal(tickStep(5 * MIN), 1);
  assert.equal(tickStep(30 * MIN), 5);
  assert.equal(tickStep(HOUR), 15, "the one-hour preset is ticked as it was");
  assert.equal(tickStep(12 * HOUR), 60);
  assert.equal(tickStep(7 * 24 * HOUR), 1440, "so is the seven-day one");
  assert.equal(tickStep(30 * 24 * HOUR), 2880);
  for (const span of [5 * MIN, HOUR, 24 * HOUR, 30 * 24 * HOUR]) {
    const ticks = span / (tickStep(span) * MIN);
    assert.ok(ticks >= 3 && ticks <= 24, `${span / MIN} minutes gives ${ticks} ticks`);
  }
});

test("only what reaches into the window is drawn", () => {
  const win = { from: 100, to: 200 };
  assert.equal(overlaps(50, 99, win), false);
  assert.equal(overlaps(50, 100, win), true);
  assert.equal(overlaps(150, 160, win), true);
  assert.equal(overlaps(200, 300, win), true);
  assert.equal(overlaps(201, 300, win), false);
});

test("ticks fall on round times of the local clock, not of UTC", () => {
  const day = 24 * HOUR;
  const from = Date.UTC(2026, 8, 24, 12);
  const to = Date.UTC(2026, 8, 27, 12);
  assert.deepEqual(tickTimes(from, to, day), [Date.UTC(2026, 8, 25), Date.UTC(2026, 8, 26), Date.UTC(2026, 8, 27)]);
  // Two hours ahead of UTC: local midnight is 22:00 UTC the day before.
  assert.deepEqual(tickTimes(from, to, day, 2 * HOUR), [
    Date.UTC(2026, 8, 24, 22), Date.UTC(2026, 8, 25, 22), Date.UTC(2026, 8, 26, 22),
  ]);
  // A tick exactly on an edge is kept; an hourly step is the same either way.
  assert.deepEqual(tickTimes(NOW, NOW + 2 * HOUR, HOUR, 2 * HOUR), [NOW, NOW + HOUR, NOW + 2 * HOUR]);
  // Five-and-a-half hours ahead (India): hourly ticks land on the half hour in UTC.
  assert.equal(tickTimes(NOW, NOW + 2 * HOUR, HOUR, 5.5 * HOUR)[0], NOW + 30 * MIN);
});
