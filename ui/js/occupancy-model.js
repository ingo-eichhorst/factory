//! How a row of the occupancy chart stacks its lanes (#120) -- pure, so it can
//! be tested without a page. The daemon decides which lane each run is in and
//! how many a row needs; this only turns those numbers into the chart's
//! geometry, and a row that never overlapped gets none of it -- it is drawn
//! exactly as a one-lane row always was.
//!
//! It also places the stretches a run spent blocked (#121) on its bar. Those
//! come from the daemon too, from the run's journal; a run that never blocked
//! has none, and its bar is drawn exactly as before.

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
