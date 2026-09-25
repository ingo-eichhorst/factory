//! How a row of the occupancy chart stacks its lanes (#120) -- pure, so it can
//! be tested without a page. The daemon decides which lane each run is in and
//! how many a row needs; this only turns those numbers into the chart's
//! geometry, and a row that never overlapped gets none of it -- it is drawn
//! exactly as a one-lane row always was.
//!
//! It also places the stretches a run spent blocked (#121) on its bar. Those
//! come from the daemon too, from the run's journal; a run that never blocked
//! has none, and its bar is drawn exactly as before.
//!
//! And it holds the chart's window (#130), as arithmetic and nothing else: no
//! DOM, no clock of its own, so every rule here is something a node test can
//! state.
//!
//! A view is one of two things, and the difference is the whole point:
//!
//! * **live** -- `{ live: true, before, after }`, milliseconds either side of
//!   now. It follows the clock: every poll and every redraw puts now in the
//!   same place. The window the select picks is one of these.
//! * **fixed** -- `{ live: false, from, to }`, epoch milliseconds. It stays
//!   where a person dragged it, and a refresh does not snap it back.
//!
//! Which one a zoom or a drag leaves behind is decided by `settle`: a window
//! that still has now in it is live, one that does not is fixed.

/// How many lanes a row is drawn with. A payload from a daemon that predates
/// lanes has none, and is one lane.
export function rowLanes(row) {
  const n = Number(row && row.lanes);
  return Number.isInteger(n) && n > 1 ? n : 1;
}

/// The inline style a row carries: `--lanes` only when there is more than one,
/// so a one-lane row's markup is what it was before lanes existed. `app.css`
/// turns it into the row's and the track's height.
export function rowStyle(row) {
  const n = rowLanes(row);
  return n > 1 ? ` style="--lanes:${n}"` : "";
}

/// The declaration a run block (and its estimate outline) adds to its style:
/// `--lane` for every lane but the top one.
export function laneStyle(block) {
  const lane = Number(block && block.lane);
  return Number.isInteger(lane) && lane > 0 ? `--lane:${lane};` : "";
}

/// `×3` while more than one run of the agent is live at once, `""` otherwise,
/// with the sentence its tooltip says.
export function concurrency(row) {
  const live = Number(row && row.live) || 0;
  if (live < 2) return null;
  return { tag: `×${live}`, title: `${live} runs of ${row.agent} are live at once` };
}

/// The stretches a run spent blocked (#121), in epoch milliseconds, clipped to
/// the run's own extent -- `to` or, while it is open, `now`. A segment still
/// open runs to the same place. Nothing for a run that never blocked, or a
/// payload from a daemon that predates segments.
export function blockedStretches(block, now) {
  const segments = Array.isArray(block && block.segments) ? block.segments : [];
  const start = new Date(block && block.from).getTime();
  const end = block && block.to ? new Date(block.to).getTime() : now;
  const out = [];
  for (const s of segments) {
    if (!s || s.status !== "blocked") continue;
    const from = Math.max(start, new Date(s.from).getTime());
    const to = Math.min(end, s.to ? new Date(s.to).getTime() : now);
    if (!(to > from)) continue;
    out.push({ from, to, seconds: (to - from) / 1000 });
  }
  return out;
}

/// What a run's tooltip adds about its waits, `""` when there were none:
/// `blocked 4m waiting for a human`, or `blocked 3× for 5m …` when it waited
/// more than once. `span` formats seconds -- the page passes `shortSpan`.
export function blockedNote(stretches, span) {
  if (!stretches.length) return "";
  const total = stretches.reduce((sum, s) => sum + s.seconds, 0);
  const times = stretches.length > 1 ? `${stretches.length}× for ` : "";
  return `blocked ${times}${span(total)} waiting for a human`;
}

/// How much of a row's busy time was spent blocked, for the util column's
/// tooltip -- `null` when none was. Busy time is not reduced by it: whether
/// a held slot waiting on a human counts as busy is still open (#121).
export function waitingShare(row) {
  const busy = Number(row && row.busy_seconds) || 0;
  const blocked = Number(row && row.blocked_seconds) || 0;
  if (busy <= 0 || blocked <= 0) return null;
  const pct = Math.min(100, Math.round((blocked / busy) * 100));
  return { pct, title: `${pct}% of the busy time was spent blocked, waiting for a human` };
}

/// The daemon's own bounds (`occupancy.rs`'s `MIN_MINUTES`/`MAX_MINUTES`).
/// Held here too so a zoom stops where the answer would, rather than
/// drawing a window the daemon then quietly narrows.
export const MIN_SPAN = 5 * 60000;
export const MAX_SPAN = 30 * 1440 * 60000;

/// Tick spacing, in minutes, for a window of up to so many minutes.
export const OCC_STEPS = [
  [15, 1], [45, 5], [2 * 60, 15], [6 * 60, 30], [12 * 60, 60], [24 * 60, 120],
  [3 * 1440, 360], [12 * 1440, 1440], [Infinity, 2 * 1440],
];

export function tickStep(spanMs) {
  const minutes = spanMs / 60000;
  return OCC_STEPS.find(([limit]) => minutes <= limit)[1];
}

/// The window the select's preset means: `minutes` back and a quarter of
/// that ahead -- the daemon's default, `window_bounds` with no from/to.
export function liveView(minutes) {
  return { live: true, before: minutes * 60000, after: Math.max(1, Math.floor(minutes / 4)) * 60000 };
}

export function resolveWindow(view, now) {
  return view.live
    ? { from: now - view.before, to: now + view.after }
    : { from: view.from, to: view.to };
}

/// What a window becomes once the gesture that made it is over.
export function settle(win, now) {
  return win.from <= now && now <= win.to
    ? { live: true, before: now - win.from, after: win.to - now }
    : { live: false, from: win.from, to: win.to };
}

/// Whether the view is the select's own window, untouched -- the only state
/// in which there is nothing to go back to.
export function isPreset(view, minutes) {
  const preset = liveView(minutes);
  return view.live && view.before === preset.before && view.after === preset.after;
}

const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));

/// Scale the window by `factor` (above 1 is out, below 1 is in) around the
/// point `fraction` of the way across it. That point stays where it was on
/// screen, which is what makes a zoom feel like it happens under the cursor.
export function zoomAround(win, factor, fraction) {
  const f = clamp(Number.isFinite(fraction) ? fraction : 0.5, 0, 1);
  const span = win.to - win.from;
  const anchor = win.from + f * span;
  const next = clamp(span * factor, MIN_SPAN, MAX_SPAN);
  const from = anchor - f * next;
  return { from, to: from + next };
}

export function pan(win, deltaMs) {
  return { from: win.from + deltaMs, to: win.to + deltaMs };
}

/// A drag of `dx` pixels over a track `width` pixels wide. Dragging right
/// pulls the past into view, the way a map moves under a hand.
export function panByPixels(win, dx, width) {
  if (!(width > 0)) return win;
  return pan(win, (-dx / width) * (win.to - win.from));
}

/// How far one wheel event zooms. A mouse wheel notch is ~100px of delta,
/// a trackpad sends many small ones, and a pinch arrives as a wheel with
/// `ctrlKey` set and deltas a tenth the size -- so it gets ten times the
/// gain. Capped per event so one flick of a free-spinning wheel does not
/// jump from an hour to a month.
export function wheelFactor(deltaY, deltaMode = 0, pinch = false) {
  const px = deltaMode === 1 ? deltaY * 16 : deltaMode === 2 ? deltaY * 400 : deltaY;
  return clamp(Math.exp(px * (pinch ? 0.01 : 0.0015)), 0.5, 2);
}

/// Where along a horizontal extent a client x falls, 0 at its left edge.
export function fractionAt(clientX, left, width) {
  return width > 0 ? clamp((clientX - left) / width, 0, 1) : 0.5;
}

/// Where a button zoom should anchor: on now when it is in view, so zooming
/// in on a live window keeps it live; otherwise the middle.
export function buttonAnchor(win, now) {
  return win.from <= now && now <= win.to ? (now - win.from) / (win.to - win.from) : 0.5;
}

/// The query for one window. `toISOString` writes `Z`: a `+01:00` offset in
/// a query string would reach the daemon as a space.
export function windowQuery(win) {
  const iso = (ms) => new Date(Math.round(ms)).toISOString();
  return `from=${encodeURIComponent(iso(win.from))}&to=${encodeURIComponent(iso(win.to))}`;
}

/// Whether anything of `[start, end]` is inside the window.
export function overlaps(start, end, win) {
  return end >= win.from && start <= win.to;
}
