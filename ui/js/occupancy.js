//! The occupancy chart. Draws what the daemon assembled and decides nothing:
//! the three layers -- runs, schedule, liveness -- arrive already separated,
//! and the only job here is to keep them visually apart.

import { $, esc, api, state, shortSpan } from "./core.js";
import { inScope, scopeLabel } from "./scopes.js";
import { openTask } from "./tasks.js";

export const OCC_STEPS = [
  [2 * 60, 15], [6 * 60, 30], [12 * 60, 60], [24 * 60, 120], [3 * 1440, 360], [Infinity, 1440],
];

export function clockLabel(d, coarse) {
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  if (!coarse) return `${hh}:${mm}`;
  return d.getHours() === 0 ? `${d.getDate()}.${d.getMonth() + 1}.` : `${hh}:${mm}`;
}


export async function loadOccupancy() {
  const minutes = Number($("occ-window").value);
  try {
    state.occ = (await api(`/api/occupancy?minutes=${minutes}`)).occupancy;
    renderOccupancy();
  } catch (e) {
    $("occ").innerHTML = `<div class="err">${esc(e.message)}</div>`;
  }
}

export function renderOccupancy() {
  const occ = state.occ;
  if (!occ) return;
  const from = new Date(occ.from).getTime();
  const to = new Date(occ.to).getTime();
  // The daemon's `now` is where the record stops. The line is drawn at the
  // wall clock so it keeps moving between polls instead of sitting still.
  const now = Math.min(to, Math.max(new Date(occ.now).getTime(), Date.now()));
  const span = Math.max(1, to - from);
  const pct = (t) => ((new Date(t).getTime() - from) / span) * 100;
  const clamp = (v) => Math.max(0, Math.min(100, v));
  const minutes = span / 60000;
  const stepMin = OCC_STEPS.find(([limit]) => minutes <= limit)[1];
  const coarse = stepMin >= 120;

  // Ticks on round clock times, not on the window's ragged edges.
  let ticks = "";
  const step = stepMin * 60000;
  for (let t = Math.ceil(from / step) * step; t <= to; t += step) {
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

      for (const b of r.blocks) {
        const l = clamp(pct(b.from));
        const w = Math.max(0.25, clamp(pct(b.to || now)) - l);
        const secs = ((b.to ? new Date(b.to) : new Date(now)) - new Date(b.from)) / 1000;
        parts.push(`<div class="blk ${esc(b.status)}${w < 6 ? " tiny" : ""}"
          data-task="${esc(b.task_id)}" data-run="${esc(b.run_id)}"
          style="left:${l.toFixed(3)}%;width:${w.toFixed(3)}%"
          title="${esc(b.title)} — attempt ${b.attempt}, ${esc(b.trigger)}, ${esc(b.status)}, ${shortSpan(secs)}">${esc(b.title)}</div>`);
      }

      for (const p of r.planned) {
        const l = clamp(pct(p.at));
        // A task that has never finished gives nothing to measure. Draw a
        // marker and say so, rather than inventing a width.
        const w = p.estimate_seconds ? Math.max(0.4, (p.estimate_seconds * 1000) / span * 100) : 0.5;
        const why = p.samples
          ? `median of ${p.samples} finished run${p.samples === 1 ? "" : "s"}`
          : "no finished run to measure";
        parts.push(`<div class="blk plan${p.samples ? "" : " noeta"}${w < 6 ? " tiny" : ""}"
          data-task="${esc(p.task_id)}"
          style="left:${l.toFixed(3)}%;width:${Math.min(w, 100 - l).toFixed(3)}%"
          title="scheduled: ${esc(p.title)} — ${why}">${esc(p.title)}</div>`);
      }

      const elapsed = Math.max(1, (now - from) / 1000);
      const busy = r.busy_seconds > 0
        ? `${Math.min(100, Math.round((r.busy_seconds / elapsed) * 100))}%`
        : (r.spans.some(s => s.status === "working") ? "—" : "idle");

      return `<div class="occ-row">
        <span class="occ-lab${r.blocks.length ? "" : " free"}"
          title="${esc(r.agent)} — ${esc(r.adapter || "no adapter")}, ${esc(r.lifetime)}, ${esc(r.role)}">
          <b>${esc(r.agent)}</b> <span class="h">${esc(r.adapter && r.adapter !== r.agent ? r.adapter : r.lifetime)}</span>
        </span>
        <span class="occ-track">${parts.join("")}</span>
        <span class="occ-util">${busy}</span>
      </div>`;
    }).join("");

    return `<div class="occ-grp">${esc(sc.name)} <span class="p">${esc(sc.path)}</span></div>
      ${rows || `<div class="occ-row"><span class="occ-lab free">no agents</span><span class="occ-track"></span><span class="occ-util"></span></div>`}`;
  }).join("");

  const nowLeft = clamp(((now - from) / span) * 100);
  // An empty chart under a selection is a fact about the scope. Falling through
  // to the instance line would claim something about the daemon that is false.
  const nothing = state.scope
    ? `No agents in ${esc(scopeLabel())}.`
    : "This instance declares no scopes.";
  const since = occ.liveness_since
    ? `Liveness has been recorded since ${new Date(occ.liveness_since).toLocaleString()}.`
    : `No liveness recorded yet — the strip stays empty until the daemon has seen an agent change state.`;

  $("occ").innerHTML = `
    <div class="occ-axis">${ticks}</div>
    <div class="occ-body">${body || `<div class="empty">${nothing}</div>`}
      <div class="occ-now" style="left:calc(var(--lab) + (100% - var(--lab) - 58px) * ${(nowLeft / 100).toFixed(4)})"><b>now</b></div>
    </div>
    <div class="occ-legend">
      <span><i style="background:var(--run)"></i>finished run</span>
      <span><i style="background:var(--wait)"></i>running</span>
      <span><i style="background:var(--fault)"></i>failed</span>
      <span><i style="background:var(--signal)"></i>blocked</span>
      <span><i style="border:1px dashed var(--signal);height:6px"></i>scheduled</span>
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
}
