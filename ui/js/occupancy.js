//! The occupancy chart. Draws what the daemon assembled and decides nothing:
//! the three layers -- runs, schedule, liveness -- arrive already separated,
//! and the only job here is to keep them visually apart.
//!
//! The window it draws is the viewer's, not the answer's: `state.occView`
//! (see `occupancy-model.js` for its two shapes) is resolved against the
//! clock on every redraw, so a drag or a zoom moves the chart at once over
//! the data already loaded, and the daemon is asked for the new window only
//! once the gesture has settled.

import { $, esc, api, state, shortSpan } from "./core.js";
import { inScope, scopeLabel } from "./scopes.js";
import { openTask } from "./tasks.js";
import {
  rowStyle, laneStyle, concurrency, blockedStretches, blockedNote, waitingShare,
  OCC_STEPS, tickStep, tickTimes, liveView, resolveWindow, settle, isPreset, zoomAround, panByPixels,
  wheelFactor, fractionAt, buttonAnchor, windowQuery, overlaps,
} from "./occupancy-model.js";

export { OCC_STEPS };

export function clockLabel(d, coarse) {
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  if (!coarse) return `${hh}:${mm}`;
  return d.getHours() === 0 ? `${d.getDate()}.${d.getMonth() + 1}.` : `${hh}:${mm}`;
}

const presetMinutes = () => Number($("occ-window").value);

/// The view, created on first use from the select, so the page opens on the
/// window it always has.
function view() {
  if (!state.occView) state.occView = liveView(presetMinutes());
  return state.occView;
}

// Only the newest request may draw: a poll that set off before a drag and
// lands after it would otherwise put the old window's data under the new.
let occSeq = 0;
let settleTimer = null;
let frame = 0;

export async function loadOccupancy() {
  // Mid-gesture nothing is fetched; the gesture's own settle will.
  if (state.occGesture) return;
  const now = Date.now();
  // A fixed window the clock has walked into starts following it.
  state.occView = settle(resolveWindow(view(), now), now);
  syncControls();
  const query = isPreset(state.occView, presetMinutes())
    ? `minutes=${presetMinutes()}`
    : windowQuery(resolveWindow(state.occView, now));
  const mine = ++occSeq;
  try {
    const occ = (await api(`/api/occupancy?${query}`)).occupancy;
    if (mine !== occSeq) return;
    state.occ = occ;
    renderOccupancy();
  } catch (e) {
    if (mine !== occSeq) return;
    $("occ").innerHTML = `<div class="err">${esc(e.message)}</div>`;
  }
}

/// Fetch once the wheel has stopped turning, not on every notch of it.
function fetchSoon() {
  clearTimeout(settleTimer);
  settleTimer = setTimeout(loadOccupancy, 250);
}

/// Redraw at most once a frame, from what is already loaded.
function drawSoon() {
  if (frame) return;
  frame = requestAnimationFrame(() => { frame = 0; renderOccupancy(); });
}

/// Replace the window with one derived from the one on screen, and redraw.
function moveView(change) {
  const now = Date.now();
  state.occView = settle(change(resolveWindow(view(), now), now), now);
  drawSoon();
  syncControls();
}

function syncControls() {
  const atPreset = isPreset(view(), presetMinutes());
  $("occ-now").disabled = atPreset;
  const range = $("occ-range");
  if (atPreset) { range.textContent = ""; return; }
  const win = resolveWindow(view(), Date.now());
  range.textContent = `${rangeLabel(new Date(win.from))} – ${rangeLabel(new Date(win.to))}${view().live ? " · following now" : ""}`;
}

function rangeLabel(d) {
  return `${d.getDate()}.${d.getMonth() + 1}. ${clockLabel(d, false)}`;
}

/// Back to the select's window, following now.
export function resetOccupancyView() {
  state.occView = liveView(presetMinutes());
  syncControls();
  renderOccupancy();
  loadOccupancy();
}

/// The horizontal extent every track shares: the axis is laid out with
/// exactly the tracks' margins, and unlike a track it is always there.
function trackExtent() {
  const axis = $("occ").querySelector(".occ-axis");
  if (!axis) return null;
  const r = axis.getBoundingClientRect();
  return r.width > 0 ? { left: r.left, width: r.width } : null;
}

/// Whether an event is over the part of the chart that moves: the axis or
/// the body, between the label column and the utilisation column.
function overTracks(e, ext) {
  if (!ext || !e.target.closest(".occ-axis, .occ-body")) return false;
  return e.clientX >= ext.left && e.clientX <= ext.left + ext.width;
}

export function wireOccupancy() {
  $("occ-window").onchange = () => resetOccupancyView();
  $("occ-now").onclick = () => resetOccupancyView();
  const zoomButton = (factor) => () => {
    moveView((win, now) => zoomAround(win, factor, buttonAnchor(win, now)));
    fetchSoon();
  };
  $("occ-zoom-in").onclick = zoomButton(0.5);
  $("occ-zoom-out").onclick = zoomButton(2);

  const el = $("occ");
  el.addEventListener("wheel", (e) => {
    const ext = trackExtent();
    if (!state.occ || !overTracks(e, ext)) return;
    e.preventDefault();
    if (Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
      // A sideways swipe on a trackpad, or shift and the wheel: a pan.
      const dx = e.deltaMode === 1 ? e.deltaX * 16 : e.deltaX;
      moveView((win) => panByPixels(win, -dx, ext.width));
    } else {
      const f = fractionAt(e.clientX, ext.left, ext.width);
      moveView((win) => zoomAround(win, wheelFactor(e.deltaY, e.deltaMode, e.ctrlKey), f));
    }
    fetchSoon();
  }, { passive: false });

  // Pointers down on the tracks, by id: one is a drag, two are a pinch.
  const down = new Map();
  let dragged = false;
  let last = null;

  const gesture = () => {
    const pts = [...down.values()];
    const ext = trackExtent();
    if (!ext || pts.length === 0) return null;
    if (pts.length === 1) return { x: pts[0].x, dist: 0, ext };
    const [a, b] = pts;
    return { x: (a.x + b.x) / 2, dist: Math.abs(a.x - b.x), ext };
  };

  el.addEventListener("pointerdown", (e) => {
    if (e.pointerType === "mouse" && e.button !== 0) return;
    if (!state.occ || !overTracks(e, trackExtent()) || down.size >= 2) return;
    down.set(e.pointerId, { x: e.clientX, x0: e.clientX });
    last = gesture();
  });

  el.addEventListener("pointermove", (e) => {
    const p = down.get(e.pointerId);
    if (!p) return;
    p.x = e.clientX;
    // A click that wobbles a pixel is still a click on a block.
    if (!state.occGesture) {
      if (down.size < 2 && Math.abs(p.x - p.x0) < 4) return;
      state.occGesture = true;
      dragged = true;
      el.classList.add("panning");
      for (const id of down.keys()) {
        try { el.setPointerCapture(id); } catch { /* already released */ }
      }
    }
    const now = gesture();
    if (!now || !last) { last = now; return; }
    moveView((win) => {
      let next = panByPixels(win, now.x - last.x, now.ext.width);
      if (now.dist > 0 && last.dist > 0) {
        next = zoomAround(next, last.dist / now.dist, fractionAt(now.x, now.ext.left, now.ext.width));
      }
      return next;
    });
    last = now;
  });

  const lift = (e) => {
    if (!down.delete(e.pointerId)) return;
    last = gesture();
    if (down.size > 0) return;
    if (state.occGesture) {
      state.occGesture = false;
      el.classList.remove("panning");
      loadOccupancy();
    }
  };
  el.addEventListener("pointerup", lift);
  el.addEventListener("pointercancel", lift);
  el.addEventListener("lostpointercapture", lift);

  // The click that ends a drag lands on whatever block is under the pointer.
  // It is the end of a gesture, not a request to open that task.
  el.addEventListener("click", (e) => {
    if (!dragged) return;
    dragged = false;
    e.stopPropagation();
    e.preventDefault();
  }, true);
  el.addEventListener("pointerdown", () => { dragged = false; }, true);

  syncControls();
}

export function renderOccupancy() {
  const occ = state.occ;
  if (!occ) return;
  // The daemon's `now` is where the record stops. The line is drawn at the
  // wall clock so it keeps moving between polls instead of sitting still.
  const now = Math.max(new Date(occ.now).getTime(), Date.now());
  const win = resolveWindow(view(), now);
  const { from, to } = win;
  // Open runs are drawn up to now, and a window that ends before now stops
  // them at its edge.
  const end = Math.min(now, to);
  const span = Math.max(1, to - from);
  const pct = (t) => ((new Date(t).getTime() - from) / span) * 100;
  const clamp = (v) => Math.max(0, Math.min(100, v));
  const stepMin = tickStep(span);
  const coarse = stepMin >= 120;

  // Ticks on round clock times, not on the window's ragged edges.
  let ticks = "";
  const shift = -new Date(from).getTimezoneOffset() * 60000;
  for (const t of tickTimes(from, to, stepMin * 60000, shift)) {
    ticks += `<span style="left:${((t - from) / span * 100).toFixed(3)}%">${clockLabel(new Date(t), coarse)}</span>`;
  }

  // Only the rows are narrowed. The window, its ticks and the now line are the
  // same clock whichever scope is being looked at, so they are left above.
  const body = occ.scopes.filter(sc => inScope(sc.name)).map(sc => {
    const rows = sc.rows.map(r => {
      const parts = [];

      for (const sp of r.spans) {
        const l = clamp(pct(sp.from));
        const w = clamp(pct(sp.to || occ.now)) - l;
        if (w <= 0) continue;
        parts.push(`<div class="liv ${esc(sp.status)}" style="left:${l.toFixed(3)}%;width:${w.toFixed(3)}%"
          title="runtime saw: ${esc(sp.status)}"></div>`);
      }

      // Runs of one agent that overlap are stacked, one lane each, as the
      // daemon packed them. The liveness strip stays along the bottom.
      for (const b of r.blocks) {
        const bEnd = b.to ? new Date(b.to).getTime() : end;
        const l = clamp(pct(b.from));
        const lane = laneStyle(b);
        if (b.estimate_seconds) {
          const expected = new Date(new Date(b.from).getTime() + b.estimate_seconds * 1000);
          const ew = clamp(pct(expected)) - l;
          if (ew > 0) {
            parts.push(`<div class="blk estimate${ew < 6 ? " tiny" : ""}"
              data-task="${esc(b.task_id)}"
              style="${lane}left:${l.toFixed(3)}%;width:${Math.min(Math.max(0.4, ew), 100 - l).toFixed(3)}%"
              title="estimated: ${esc(b.title)} — user estimate ${shortSpan(b.estimate_seconds)}, expected until ${esc(expected.toLocaleString())}">${esc(b.title)}</div>`);
          }
        }
        if (!overlaps(new Date(b.from).getTime(), bEnd, win)) continue;
        const w = Math.max(0.25, clamp(pct(b.to || end)) - l);
        const secs = ((b.to ? new Date(b.to) : new Date(end)) - new Date(b.from)) / 1000;
        const waits = blockedStretches(b, end);
        const note = blockedNote(waits, shortSpan);
        parts.push(`<div class="blk run ${esc(b.status)}${w < 6 ? " tiny" : ""}"
          data-task="${esc(b.task_id)}" data-run="${esc(b.run_id)}"
          style="${lane}left:${l.toFixed(3)}%;width:${w.toFixed(3)}%"
          title="${esc(b.title)} — attempt ${b.attempt}, ${esc(b.trigger)}, ${esc(b.status)}, ${shortSpan(secs)}${note ? ` · ${note}` : ""}">${esc(b.title)}</div>`);
        // The time it spent waiting on a human, laid over the bar in the run's
        // own lane. The bar keeps the colour of how the run ended; this is
        // what it waited on along the way, which the run record forgets.
        for (const s of waits) {
          const sl = clamp(pct(s.from));
          const sw = clamp(pct(s.to)) - sl;
          if (sw <= 0) continue;
          parts.push(`<div class="blk seg blocked" data-task="${esc(b.task_id)}" data-run="${esc(b.run_id)}"
            style="${lane}left:${sl.toFixed(3)}%;width:${Math.max(0.25, sw).toFixed(3)}%"
            title="${esc(b.title)} — blocked ${shortSpan(s.seconds)} waiting for a human"></div>`);
        }
      }

      for (const p of r.planned) {
        const at = new Date(p.at).getTime();
        if (!overlaps(at, at + (p.estimate_seconds || 0) * 1000, win)) continue;
        const l = clamp(pct(p.at));
        // A task that has never finished gives nothing to measure. Draw a
        // marker and say so, rather than inventing a width.
        const w = p.estimate_seconds ? Math.max(0.4, (p.estimate_seconds * 1000) / span * 100) : 0.5;
        const why = p.user_estimate
          ? "user estimate"
          : (p.samples
            ? `median of ${p.samples} finished run${p.samples === 1 ? "" : "s"}`
            : "no estimate and no finished run to measure");
        parts.push(`<div class="blk plan${p.estimate_seconds ? "" : " noeta"}${w < 6 ? " tiny" : ""}"
          data-task="${esc(p.task_id)}"
          style="left:${l.toFixed(3)}%;width:${Math.min(w, 100 - l).toFixed(3)}%"
          title="scheduled: ${esc(p.title)} — ${why}">${esc(p.title)}</div>`);
      }

      // Utilisation is the answer's, over the window it was asked for --
      // not whatever the view has been dragged to since.
      const asked = new Date(occ.from).getTime();
      const elapsed = Math.max(1, (Math.min(now, new Date(occ.to).getTime()) - asked) / 1000);
      const busy = asked > now ? "—" : r.busy_seconds > 0
        ? `${Math.min(100, Math.round((r.busy_seconds / elapsed) * 100))}%`
        : (r.spans.some(s => s.status === "working") ? "—" : "idle");
      // Blocked time stays inside busy time (#121 leaves that call open); the
      // share of it spent waiting is marked on the figure, not taken off it.
      const wait = waitingShare(r);
      const shown = wait ? `<span class="occ-wait" title="${esc(wait.title)}">${busy}</span>` : busy;
      const conc = concurrency(r);
      const util = conc
        ? `<span class="occ-conc" title="${esc(conc.title)}">${esc(conc.tag)}</span> ${shown}`
        : shown;

      return `<div class="occ-row"${rowStyle(r)}>
        <span class="occ-lab${r.blocks.length ? "" : " free"}"
          title="${esc(r.agent)} — ${esc(r.adapter || "no adapter")}, ${esc(r.lifetime)}, ${esc(r.role)}">
          <b>${esc(r.agent)}</b> <span class="h">${esc(r.adapter && r.adapter !== r.agent ? r.adapter : r.lifetime)}</span>
        </span>
        <span class="occ-track">${parts.join("")}</span>
        <span class="occ-util">${util}</span>
      </div>`;
    }).join("");

    return `<div class="occ-grp">${esc(sc.name)} <span class="p">${esc(sc.path)}</span></div>
      ${rows || `<div class="occ-row"><span class="occ-lab free">no agents</span><span class="occ-track"></span><span class="occ-util"></span></div>`}`;
  }).join("");

  // Off the window, there is no now line: pinned to an edge it would claim a
  // moment that is not there.
  const nowLeft = ((now - from) / span) * 100;
  const nowLine = nowLeft >= 0 && nowLeft <= 100
    ? `<div class="occ-now" style="left:calc(var(--lab) + (100% - var(--lab) - 58px) * ${(nowLeft / 100).toFixed(4)})"><b>now</b></div>`
    : "";
  // An empty chart under a selection is a fact about the scope. Falling through
  // to the instance line would claim something about the daemon that is false.
  const nothing = state.scope
    ? `No agents in ${esc(scopeLabel())}.`
    : "This instance has no configured scopes.";
  const since = occ.liveness_since
    ? `Liveness has been recorded since ${new Date(occ.liveness_since).toLocaleString()}.`
    : `No liveness recorded yet — the strip stays empty until the daemon has seen an agent change state.`;

  $("occ").innerHTML = `
    <div class="occ-axis">${ticks}</div>
    <div class="occ-body">${body || `<div class="empty">${nothing}</div>`}
      ${nowLine}
    </div>
    <div class="occ-legend">
      <span><i style="background:var(--run)"></i>finished run</span>
      <span><i style="background:var(--wait)"></i>running</span>
      <span><i style="background:var(--fault)"></i>failed</span>
      <span><i style="background:var(--signal)"></i>blocked</span>
      <span><i style="background:var(--run);box-shadow:inset 0 -3px 0 var(--signal)"></i>waited on a human</span>
      <span><i style="border:1px dashed var(--signal);height:6px"></i>scheduled</span>
      <span><i style="border:1px dotted var(--wait);height:6px"></i>estimated duration</span>
      <span><i style="background:var(--run);opacity:.6;height:4px"></i>runtime saw it busy</span>
    </div>
    <div class="occ-note"><b>Two kinds of fact.</b> A block is a run: Factory started it and the agent
      reported back. The thin strip is only what the runtime saw on the screen, which is a guess about
      a terminal, not a claim about work. ${esc(since)} herdr keeps no history of its own, so nothing
      before that exists.</div>`;

  for (const el of $("occ").querySelectorAll(".blk[data-run]")) {
    el.onclick = () => openTask(el.dataset.task, el.dataset.run);
  }
  for (const el of $("occ").querySelectorAll(".blk.plan")) {
    el.onclick = () => openTask(el.dataset.task);
  }
  for (const el of $("occ").querySelectorAll(".blk.estimate")) {
    el.onclick = () => openTask(el.dataset.task);
  }
}
