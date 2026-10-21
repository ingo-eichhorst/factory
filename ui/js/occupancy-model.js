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
//!
//! And it holds what makes a standing agent visible (#331): its sessions'
//! lanes and uptime bands, the util column that never calls an agent that is
//! up `idle`, the slots it holds, and which rows the "standing only" filter
//! keeps. The daemon assembled all of it; this only shapes it for the page.

/// The lanes a row's runs need, as the daemon packed them. A payload from a
/// daemon that predates lanes has none, and is one lane.
export function blockLanes(row) {
  const n = Number(row && row.lanes);
  return Number.isInteger(n) && n > 1 ? n : 1;
}

/// A standing row's sessions, one lane each (#331). Nothing for a task agent
/// or a payload from a daemon that predates them.
export function sessionsOf(row) {
  return Array.isArray(row && row.sessions) ? row.sessions : [];
}

/// Where a row's runs start: below its sessions' lanes, so a run of the same
/// agent name -- another session -- is never drawn over the standing one.
export function blockOffset(row) {
  return sessionsOf(row).length;
}

/// How many lanes a row is drawn with: one per session, then as many as its
/// runs need -- or none for them, when a standing row has no run or schedule
/// in the window. A row with neither is the one lane it always was.
export function rowLanes(row) {
  const sessions = sessionsOf(row).length;
  if (!sessions) return blockLanes(row);
  const work = (row.blocks && row.blocks.length) || (row.planned && row.planned.length);
  return sessions + (work ? blockLanes(row) : 0);
}

/// The inline style a row carries: `--lanes` only when there is more than one,
/// so a one-lane row's markup is what it was before lanes existed. `app.css`
/// turns it into the row's and the track's height.
export function rowStyle(row) {
  const n = rowLanes(row);
  return n > 1 ? ` style="--lanes:${n}"` : "";
}

/// The declaration a run block (and its estimate outline) adds to its style:
/// `--lane` for every lane but the top one. `offset` is the lanes above the
/// runs (`blockOffset`): a standing row's sessions.
export function laneStyle(block, offset = 0) {
  const lane = Number(block && block.lane);
  const base = Number.isInteger(offset) && offset > 0 ? offset : 0;
  const n = (Number.isInteger(lane) && lane > 0 ? lane : 0) + base;
  return n > 0 ? `--lane:${n};` : "";
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

/// Tick times across `[from, to]`, every `stepMs`, on round times of the
/// local clock: `shiftMs` is how far that clock is ahead of UTC (minus
/// `getTimezoneOffset`, in milliseconds). Without it a daily tick lands on
/// UTC midnight, which in Berlin is labelled 02:00. One shift for the whole
/// window -- the day the clock changes is off by an hour, and says so by
/// its label rather than by drawing a tick in the wrong place.
export function tickTimes(from, to, stepMs, shiftMs = 0) {
  const out = [];
  for (let t = Math.ceil((from + shiftMs) / stepMs) * stepMs - shiftMs; t <= to; t += stepMs) out.push(t);
  return out;
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
  if (!Number.isFinite(width) || width <= 0) return win;
  return pan(win, (-dx / width) * (win.to - win.from));
}

/// Pixels in one unit of a wheel event's `deltaMode`: pixels, lines, pages.
const WHEEL_UNIT_PX = [1, 16, 400];

/// How far one wheel event zooms. A mouse wheel notch is ~100px of delta,
/// a trackpad sends many small ones, and a pinch arrives as a wheel with
/// `ctrlKey` set and deltas a tenth the size -- so it gets ten times the
/// gain. Capped per event so one flick of a free-spinning wheel does not
/// jump from an hour to a month.
export function wheelFactor(deltaY, deltaMode = 0, pinch = false) {
  const px = deltaY * (WHEEL_UNIT_PX[deltaMode] ?? 1);
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

// -- standing agents (#331) ---------------------------------------------------

/// A standing agent: one with a session of its own, permanent or temporary.
export function isStanding(row) {
  return !!row && (row.lifetime === "permanent" || row.lifetime === "temporary");
}

/// The rows of one scope group, split for drawing: standing first, each in
/// the order it came in. The daemon already sends them that way; this keeps
/// an older daemon's answer in the same shape. With `standingOnly` there are
/// no task rows at all.
export function groupRows(rows, standingOnly = false) {
  const list = Array.isArray(rows) ? rows : [];
  return {
    standing: list.filter(isStanding),
    task: standingOnly ? [] : list.filter((r) => !isStanding(r)),
  };
}

/// The scope groups the chart draws: those `keep` (the scope selection) lets
/// through, each with its rows grouped. Under `standingOnly` a group with no
/// standing agent is left out -- an empty heading says nothing.
export function visibleGroups(scopes, keep, standingOnly = false) {
  const list = Array.isArray(scopes) ? scopes : [];
  return list
    .filter((sc) => !keep || keep(sc.name))
    .map((sc) => ({ scope: sc, ...groupRows(sc.rows, standingOnly) }))
    .filter((g) => !standingOnly || g.standing.length > 0);
}

const STATES = new Set(["ready", "starting", "gone", "stopped"]);

/// The class of a standing row's state dot: its roster state, when it is one
/// of the four a standing agent has; `""` otherwise.
export function stateClass(row) {
  return row && STATES.has(row.state) ? row.state : "";
}

/// The badge on a standing row's label: its lifetime, and the short form a
/// narrow screen shows. `null` for a task agent.
export function lifetimeBadge(row) {
  if (!isStanding(row)) return null;
  return {
    text: row.lifetime,
    short: row.lifetime === "permanent" ? "perm" : "temp",
    title: row.lifetime === "permanent"
      ? "permanent: Factory keeps its session up and restarts it when it goes"
      : "temporary: started on demand, not kept up",
  };
}

/// One session's uptime as bands to draw, in epoch milliseconds. A stretch
/// still up runs to `now`. Each band carries how it ended, `null` while up.
export function uptimeBands(session, now) {
  const uptime = Array.isArray(session && session.uptime) ? session.uptime : [];
  const out = [];
  for (const u of uptime) {
    const from = new Date(u.from).getTime();
    const to = u.to ? new Date(u.to).getTime() : now;
    if (!(to > from)) continue;
    out.push({ from, to, ended: u.to ? (u.ended || "gone") : null });
  }
  return out;
}

const ENDINGS = {
  restarted: "went away and came back up",
  gone: "went away and has not been seen since",
  stopped: "stopped by a person",
};

/// What a band's tooltip says. `when` formats epoch milliseconds, `span`
/// seconds -- the page passes its own. A stretch the daemon opened at the
/// window's start (`windowFrom`) was up before it, for however long the
/// record does not say here.
export function uptimeNote(band, when, span, windowFrom = -Infinity) {
  const before = band.from <= windowFrom;
  const start = before ? "before this window" : when(band.from);
  const length = before ? `${span((band.to - band.from) / 1000)} of it in view` : span((band.to - band.from) / 1000);
  if (!band.ended) return `up since ${start} (${length}), and up now`;
  return `up ${before ? "from " : ""}${start} – ${when(band.to)} (${length}), then ${ENDINGS[band.ended] || band.ended}`;
}

/// The marks on a session's lane: where each stretch ended, and how.
export function uptimeMarks(bands) {
  return bands.filter((b) => b.ended).map((b) => ({ at: b.to, kind: b.ended, title: ENDINGS[b.ended] || b.ended }));
}

/// The util column of a standing row: how much of the window it was up and
/// what it is doing now -- `up 100% · ready` -- never `idle` for an agent
/// that is up. `elapsed` is the seconds of the window the answer covers; with
/// no uptime recorded in it the state is all there is to say (`up` is then
/// `null`). `null` for a task agent, whose column is its busy share as before.
export function standingUtil(row, elapsed) {
  if (!isStanding(row)) return null;
  const state = row.state || "unknown";
  const seconds = Number(row.up_seconds) || 0;
  const share = elapsed > 0 ? Math.min(100, Math.round((seconds / elapsed) * 100)) : 0;
  const up = seconds > 0 ? `up ${share}%` : null;
  const text = up ? `${up} · ${state}` : state;
  const busy = Number(row.busy_seconds) || 0;
  const runs = busy > 0 && elapsed > 0 ? Math.min(100, Math.round((busy / elapsed) * 100)) : null;
  const title = up
    ? `its session was up ${share}% of the window and is ${state} now`
    : `no uptime recorded in this window; it is ${state} now`;
  return { text, up, state, runs, title };
}

/// The slots a row holds against its `max_sessions`, as admission and
/// `factory status` count them: `1/5`, or `1 slot` with no cap. `null` when the
/// daemon sent none, or for a task agent holding nothing.
export function slotsLabel(row) {
  const slots = row && row.slots;
  if (!slots) return null;
  const used = Number(slots.in_use) || 0;
  const max = Number.isInteger(slots.max) && slots.max > 0 ? slots.max : null;
  if (!isStanding(row) && used === 0) return null;
  return {
    text: max ? `${used}/${max}` : `${used} slot${used === 1 ? "" : "s"}`,
    title: max
      ? `holds ${used} of its ${max} sessions (max_sessions), as admission counts them`
      : `holds ${used} session${used === 1 ? "" : "s"}; no max_sessions`,
    full: !!max && used >= max,
  };
}

/// What a session's lane is called in its tooltips when its row has more
/// than one: its subject, the id the liveness record keeps it under. `""`
/// for a row with one session -- the row's name already says it.
export function sessionLabel(row, session, index) {
  if (sessionsOf(row).length < 2) return "";
  const subject = String((session && session.subject) || "");
  return subject || `session ${index + 1}`;
}
