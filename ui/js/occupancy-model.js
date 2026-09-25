//! How a row of the occupancy chart stacks its lanes (#120) -- pure, so it can
//! be tested without a page. The daemon decides which lane each run is in and
//! how many a row needs; this only turns those numbers into the chart's
//! geometry, and a row that never overlapped gets none of it -- it is drawn
//! exactly as a one-lane row always was.

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
