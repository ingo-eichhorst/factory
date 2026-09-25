//! The L4 Process Operations tab (`#106`): how the line is running, read
//! from `GET /api/operations` -- the same report `factory stats` prints and
//! the Dashboard's Inbox reads its list from. `operations-model.js` holds
//! every pure shaping function this file draws from; this file only
//! fetches, renders and wires clicks, the split `scenarios.js` keeps.
//!
//! Top to bottom, exception first: what changed since you last looked, what
//! needs a person, how work is flowing, which work is running late while it
//! is still running (Vacanti's Aging WIP), whether the last 7 or 30 days
//! were healthy, and the schedules. A normal day is almost empty at the top.
//!
//! Rail semantics, and why there are two reads. The daemon narrows a report
//! to one scope by exact name, not to the subtree the rail means everywhere
//! else. Attention, flow, aging and schedules are per-scope rows the
//! unscoped report already carries, so they are narrowed here with the
//! rail's own `inScope` -- a parent scope shows its children's trouble, as
//! it does on every other tab. Health is a set of window-wide figures that
//! cannot be added up across scopes afterwards, so with a scope selected it
//! comes from a second, scoped read and says it covers that scope alone --
//! the same caveat `production.rs` makes for the dashboard's history cards.
//!
//! Every action goes through a confirmation in the app's own modal (never
//! `confirm()`), carries an optional reason -- a required one for an answer
//! -- and is journaled by the daemon with who asked and why. Nothing here
//! starts, stops or retries anything on its own: a picture, not a
//! controller (design §8).

import { $, api, esc, state } from "./core.js";
import { inScope, routeHref, scopeLabel } from "./scopes.js";
import { scrim, closeModal, dropModal } from "./modal.js";
import { openTask } from "./tasks.js";
import { describeWorkflowOrigin } from "./workflows.js";
import {
  MULTIPLES,
  PACES,
  PACE_LABELS,
  SEEN_KEY,
  STAGES,
  TILES,
  actionConsequence,
  actionLabel,
  actionReady,
  actionRequest,
  agingByScope,
  agingGeometry,
  attentionRows,
  basisText,
  blockedWorkflowNodes,
  bulkOffers,
  bulkPreview,
  capacityText,
  cfdGeometry,
  dayLabel,
  failKindRows,
  figureText,
  flowBars,
  fmtAge,
  kindLabel,
  multipleGeometry,
  readSeen,
  scatterGeometry,
  scheduleActions,
  scheduleRows,
  seenSnapshot,
  sinceLastLooked,
  trend,
  utcStamp,
} from "./operations-model.js";

// ------------------------------------------------------------------- state
//
// Module-private, like `scenarios.js`: nothing else reads it.

let report = null;       // the unscoped report: attention, flow, aging, schedules
let health = null;       // the report health is read from -- `report`, or the scoped one
let reportError = null;
let healthError = null;
let asked = 0;
let windowKey = "7d";    // "7d" | "30d"
let showScatter = false;
let showCfd = false;

// "Since you last looked": what this browser remembered when the tab was
// last left, read once as the tab is shown so every live refresh while it
// stays open is compared with the same moment.
let seenBaseline = null;
let visible = false;

function storageRead() {
  try { return readSeen(localStorage.getItem(SEEN_KEY)); } catch { return null; }
}

function storageWrite(snapshot) {
  try { localStorage.setItem(SEEN_KEY, JSON.stringify(snapshot)); } catch { /* private window */ }
}

/// Remember what is on screen as "last looked". Only once a report has
/// actually been seen -- a tab left before its first answer arrived has
/// shown nothing, and writing that would read as everything resolved.
function rememberSeen() {
  if (!report) return;
  storageWrite(seenSnapshot(currentRows(), Date.now(), storageRead()));
}

function currentRows() {
  return attentionRows(report, inScope, Date.now());
}

// ------------------------------------------------------------------ loading

export async function loadOperations() {
  const mine = ++asked;
  const unscoped = api(`/api/operations?window=${windowKey}`);
  const scoped = state.scope === null
    ? null
    : api(`/api/operations?window=${windowKey}&scope=${encodeURIComponent(state.scope)}`);
  const [all, one] = await Promise.allSettled([unscoped, scoped || Promise.resolve(null)]);
  if (mine !== asked) return;
  if (all.status === "fulfilled") { report = all.value.report; reportError = null; }
  else { report = null; reportError = all.reason.message; }
  if (state.scope === null) { health = report; healthError = reportError; }
  else if (one.status === "fulfilled") { health = one.value.report; healthError = null; }
  else { health = null; healthError = one.reason.message; }
  renderOperations();
}

/// `onShow`: read what this browser saw last, then fetch.
export function showOperations() {
  visible = true;
  seenBaseline = storageRead();
  loadOperations();
}

/// `onHide`: this is now what was last looked at.
export function hideOperations() {
  if (visible) rememberSeen();
  visible = false;
}

// ------------------------------------------------------------------ render

export function renderOperations() {
  const host = $("ops");
  if (!host) return;
  const failed = $("ops-error");
  if (failed) { failed.textContent = reportError || ""; failed.hidden = !reportError; }
  const stamp = $("ops-generated");
  if (stamp) stamp.textContent = report ? `as of ${utcStamp(report.generated_at)}` : "";
  if (!report) {
    host.innerHTML = reportError ? "" : `<div class="empty">loading…</div>`;
    return;
  }
  const now = Date.now();
  const rows = attentionRows(report, inScope, now);
  // Keep the open `<details>` where the reader left them across a redraw.
  host.innerHTML = `
    ${sinceStripHtml(rows)}
    ${attentionHtml(rows)}
    ${flowHtml()}
    ${agingHtml(now)}
    ${healthHtml()}
    ${schedulesHtml()}`;
  wire(host, rows);
}

// --------------------------------------------------- since you last looked

function sinceStripHtml(rows) {
  const finishedRuns = ((report.health && report.health.current.finished_runs) || [])
    .filter((r) => {
      const t = state.tasks.get(r.task_id);
      return t ? inScope(t.scope) : state.scope === null;
    });
  const d = sinceLastLooked(seenBaseline, rows, finishedRuns, report.health && report.health.current.window.from);
  if (d.first) {
    return `<p class="ops-since">First look from this browser -- from the next visit this line says what changed since.</p>`;
  }
  const part = (n, one, many) => `<b>${n}</b> ${n === 1 ? one : many}`;
  return `<p class="ops-since" title="Kept in this browser only; another browser or a private window starts fresh.">
    Since you last looked (${esc(utcStamp(d.at))}): ${part(d.fresh, "new exception", "new exceptions")},
    ${part(d.resolved, "resolved", "resolved")}, ${part(d.finished, "run finished", "runs finished")}${d.beyondWindow ? ` <span class="sub">(finished counts only reach back ${esc(windowKey)})</span>` : ""}.</p>`;
}

// ----------------------------------------------------------- needs attention

function actionButtons(row, actions) {
  return actions
    .map((a) => `<button type="button" class="btn ops-act${a === "cancel" ? " danger" : ""}" data-act="${esc(a)}" data-key="${esc(row.key)}">${esc(actionLabel(a))}</button>`)
    .join("");
}

/// Where a row leads when it is clicked: its run in the task modal, or for
/// the two kinds with no task, the tab that owns them.
function rowTarget(r) {
  if (r.task_id) return "task";
  if (r.kind === "liveness_lost") return routeHref(state.scope, "roster");
  if (r.kind === "triggered_signpost") return routeHref(state.scope, "scenarios");
  return "";
}

function attentionRowHtml(r) {
  const target = rowTarget(r);
  const who = r.title || r.agent || "";
  // A suspicion is a guess from a screen read or a quiet journal, and the
  // rule is it may be shown, never asserted -- so it is words beside the
  // kind, never a status badge, and never the status colour of `blocked`.
  const flag = r.suspicion
    ? `<span class="ops-suspect" title="A screen read or a long silence -- not a status the agent reported.">suspicion, not a status</span>`
    : r.observation ? `<span class="ops-suspect">observation</span>` : "";
  const open = target === "task"
    ? `data-task="${esc(r.task_id)}"${r.run_id ? ` data-run="${esc(r.run_id)}"` : ""}`
    : target ? `data-href="${esc(target)}"` : "";
  return `<div class="ops-row" ${open} tabindex="0" role="button" aria-label="${esc(`${r.label}: ${who}`)}">
    <span class="it-dot" style="background:var(--${r.tone})"></span>
    <div class="ops-main">
      <div class="ops-head"><span class="ops-kind k-${esc(r.severity)}">${esc(r.label)}</span>${flag}
        <b>${esc(who)}</b>${r.scope ? `<span class="sub">${esc(r.scope)}</span>` : ""}</div>
      <div class="ops-reason">${esc(r.reason)}</div>
      ${r.alsoLabels.length ? `<div class="sub">also ${esc(r.alsoLabels.join(", "))}${(r.also || []).includes("suspected_stuck") ? ` <span class="ops-suspect">(may be stuck is a suspicion, not a status)</span>` : ""}</div>` : ""}
    </div>
    <span class="it-age" title="since ${esc(utcStamp(r.since))}">${esc(fmtAge(r.age))}</span>
    ${r.actions && r.actions.length ? `<div class="ops-acts">${actionButtons(r, r.actions)}</div>` : ""}
  </div>`;
}

function attentionHtml(rows) {
  const offers = bulkOffers(rows);
  const bulk = offers
    .map((p) => `<button type="button" class="btn ops-bulk" data-bulk="${esc(p.action)}">${esc(actionLabel(p.action))} all ${p.count}…</button>`)
    .join("");
  let body;
  if (!rows.length) {
    const seen = seenBaseline && seenBaseline.last_exception_at;
    body = `<div class="empty ops-calm">Nothing needs you.${seen
      ? ` <span class="sub">The last exception this browser saw began ${esc(utcStamp(seen))}.</span>`
      : ""}</div>`;
  } else {
    body = `<div class="ops-list">${rows.map(attentionRowHtml).join("")}</div>`;
  }
  return `<section class="dcard ops-card">
    <h3>Needs attention<span class="r">${rows.length} · by severity, then age</span></h3>
    ${bulk ? `<div class="row-btns ops-bulkrow">${bulk}</div>` : ""}
    ${body}
  </section>`;
}

// ------------------------------------------------------------------ flow now

function tableBlock(title, head, bodyRows) {
  if (!bodyRows.length) return "";
  return `<details class="ops-table"><summary>${esc(title)}</summary><div class="ops-tscroll"><table>
    <thead><tr>${head.map((h) => `<th>${esc(h)}</th>`).join("")}</tr></thead>
    <tbody>${bodyRows.map((r) => `<tr>${r.map((c) => `<td>${c}</td>`).join("")}</tr>`).join("")}</tbody>
  </table></div></details>`;
}

function figureCell(fig) {
  const text = figureText(fig, fmtAge);
  return fig && fig.reason ? `<span title="${esc(fig.reason)}">${esc(text)}${fig.value === null || fig.value === undefined ? "" : "*"}</span>` : esc(text);
}

function flowHtml() {
  const bars = flowBars(report.flow, inScope);
  const occ = routeHref(state.scope, "occupancy");
  if (!bars.length) {
    return `<section class="dcard ops-card"><h3>Flow now<span class="r">nothing in flight, queued or retrying</span></h3>
      <div class="empty">The line is idle.</div></section>`;
  }
  const legend = STAGES.map((s) => `<span><i class="st-${s}"></i>${esc(s)}</span>`).join("");
  const rows = bars.map((f) => `<div class="ops-flow">
      <div class="ops-flow-top"><b>${esc(f.scope)}</b><span class="sub">${f.total} in flight</span></div>
      <div class="ops-stack" role="img" aria-label="${esc(STAGES.map((s) => `${f.wip[s]} ${s}`).join(", "))}">
        ${f.segments.map((s) => `<i class="st-${s.stage}" style="left:${s.x.toFixed(2)}%;width:${s.w.toFixed(2)}%" title="${s.n} ${esc(s.stage)}"></i>`).join("")}
      </div>
      <div class="ops-flow-stats">
        <span>queue <b>${f.queue_depth}</b></span>
        <span>wait p50 <b>${figureCell(f.wait_p50)}</b> · p95 <b>${figureCell(f.wait_p95)}</b></span>
        <span><a href="${esc(occ)}" title="Occupancy has the runs and sessions behind this">sessions ${esc(capacityText(f))}</a></span>
        <span${f.retrying ? ` class="warn"` : ""}>retrying <b>${f.retrying}</b></span>
      </div>
    </div>`).join("");
  const table = tableBlock("Flow as a table", ["Scope", ...STAGES, "Wait p50", "Wait p95", "Sessions", "Retrying"],
    bars.map((f) => [esc(f.scope), ...STAGES.map((s) => String(f.wip[s])), figureCell(f.wait_p50), figureCell(f.wait_p95), esc(capacityText(f)), String(f.retrying)]));
  return `<section class="dcard ops-card">
    <h3>Flow now<span class="r">work in flight by state, per scope · waits over ${esc(windowKey)}</span></h3>
    <div class="chleg">${legend}</div>
    ${rows}
    <p class="dnote">Capacity is unknown while <code>max_sessions</code> has no effect, so no utilisation is drawn. A retrying task
      is failing with retries left -- it is counted here and only reaches Needs attention once they run out.
      * a wait marked so leaves out runs from before queue waits were recorded.</p>
    ${table}
  </section>`;
}

// ------------------------------------------------------------------ aging WIP

function agingChartHtml(group, elapsed) {
  const g = agingGeometry(group.items, group.lines, elapsed, { width: 100, height: 240 });
  const lines = g.lines.map((l) => `<line x1="0" x2="100" y1="${l.y.toFixed(1)}" y2="${l.y.toFixed(1)}" class="ops-pline ops-${l.name}"/>`).join("");
  const grid = g.ticks.map((t) => `<line x1="0" x2="100" y1="${t.y.toFixed(1)}" y2="${t.y.toFixed(1)}" class="ops-grid"/>`).join("");
  const cols = g.columns.slice(1).map((c) => `<line x1="${c.x}" x2="${c.x}" y1="0" y2="240" class="ops-grid"/>`).join("");
  const yLabels = g.ticks.map((t) => `<span style="top:${t.y.toFixed(1)}px">${esc(t.label)}</span>`).join("");
  const lineLabels = g.lines.map((l) => `<span class="ops-plabel" style="top:${l.labelY.toFixed(1)}px">${esc(l.name)} ${esc(fmtAge(l.v))}</span>`).join("");
  const dots = g.dots.map((d) => {
    const pace = d.pace ? PACE_LABELS[d.pace] : "no pace";
    const title = `${d.title} · ${d.stage} ${fmtAge(d.age)} · ${pace} (${basisText(d.basis)})`;
    return `<button type="button" class="ops-dot ${d.paceClass}" style="left:${d.cx.toFixed(2)}%;top:${d.cy.toFixed(1)}px"
      data-task="${esc(d.task_id)}"${d.run_id ? ` data-run="${esc(d.run_id)}"` : ""} title="${esc(title)}" aria-label="${esc(title)}"></button>`;
  }).join("");
  const xLabels = g.columns.map((c) => `<span style="left:${c.cx}%">${esc(c.stage)}</span>`).join("");
  return `<div class="ops-aging">
    <div class="ops-aging-head"><b>${esc(group.scope)}</b>
      <span class="sub">${group.lines ? `lines from ${group.lines.samples} finished runs` : "not enough history -- fewer than 5 finished runs, so no lines and no pace colour"}</span></div>
    <div class="ops-plot-row">
      <div class="ops-yaxis">${yLabels}</div>
      <div class="ops-plot" style="height:240px">
        <svg viewBox="0 0 100 240" preserveAspectRatio="none" aria-hidden="true">${grid}${cols}${lines}</svg>
        ${lineLabels}${dots}
      </div>
    </div>
    <div class="ops-xaxis">${xLabels}</div>
  </div>`;
}

function agingHtml(now) {
  const groups = agingByScope(report.aging, inScope);
  const elapsed = (now - Date.parse(report.generated_at)) / 1000;
  const legend = PACES.map((p) => `<span><i class="pace-${p}"></i>${esc(PACE_LABELS[p])}</span>`).join("")
    + `<span><i class="pace-none"></i>no pace</span>`;
  const all = groups.flatMap((g) => g.items);
  const table = tableBlock("Aging WIP as a table", ["Task", "Scope", "Stage", "Age", "Pace", "Judged"],
    all.map((it) => [esc(it.title), esc(it.scope), esc(it.stage), esc(fmtAge(it.age_s + Math.max(0, elapsed))), esc(it.pace ? PACE_LABELS[it.pace] : "—"), esc(basisText(it.basis))]));
  return `<section class="dcard ops-card">
    <h3>Aging WIP<span class="r">age of work still in progress against past cycle times · log scale</span></h3>
    ${groups.length ? `<div class="chleg">${legend}</div>${groups.map((g) => agingChartHtml(g, elapsed)).join("")}` : `<div class="empty">Nothing is in progress.</div>`}
    ${groups.length ? `<p class="dnote">Dashed lines are the scope's p50, p70, p85 and p95 cycle times. A dot's colour is judged against
      its own task's history when it has five finished runs, else its scope's -- hover a dot to see which. Click one to open its run.</p>` : ""}
    ${table}
  </section>`;
}

// ------------------------------------------------------------------ health

function multipleHtml(m, cur, prev) {
  const g = multipleGeometry(cur.days, prev.days, m.of, { width: 100, height: 44 });
  const t = trend(cur[m.fig], prev[m.fig], m.better);
  const pts = (runs, cls) => runs.map((r) => r.length === 1
    ? `<circle cx="${r[0][0]}" cy="${r[0][1]}" r="1.4" class="${cls}"/>`
    : `<polyline points="${r.map((p) => p.join(",")).join(" ")}" class="${cls}"/>`).join("");
  const arrow = { up: "▲", down: "▼", flat: "→", none: "" }[t.dir];
  return `<div class="ops-mult">
    <div class="k">${esc(m.title)}</div>
    <div class="v">${esc(figureText(cur[m.fig], m.fmt))}<span class="d ${t.tone}">${arrow ? `${arrow} ` : ""}prev ${esc(figureText(prev[m.fig], m.fmt))}</span></div>
    ${g.empty ? `<div class="ops-mult-empty">nothing finished</div>` : `<svg viewBox="0 0 100 44" preserveAspectRatio="none" role="img"
      aria-label="${esc(`${m.title} per day, this window against the one before`)}">${pts(g.previous, "ghost")}${pts(g.current, "cur")}</svg>`}
  </div>`;
}

function tileHtml(tl, cur, prev) {
  const t = trend(cur[tl.fig], prev[tl.fig], tl.better);
  const arrow = { up: "▲", down: "▼", flat: "→", none: "" }[t.dir];
  const reason = cur[tl.fig] && cur[tl.fig].reason;
  return `<div class="ops-mult"${reason ? ` title="${esc(reason)}"` : ""}>
    <div class="k">${esc(tl.title)}</div>
    <div class="v">${esc(figureText(cur[tl.fig], tl.fmt))}<span class="d ${t.tone}">${arrow ? `${arrow} ` : ""}prev ${esc(figureText(prev[tl.fig], tl.fmt))}</span></div>
    ${reason ? `<div class="ops-mult-empty">${esc(reason)}</div>` : ""}
  </div>`;
}

function scatterHtml(cur) {
  const g = scatterGeometry(cur.finished_runs, cur.window, cur, { width: 100, height: 200 });
  if (g.empty) return `<div class="empty">No run ended done in this window.</div>`;
  const lines = g.lines.map((l) => `<line x1="0" x2="100" y1="${l.y.toFixed(1)}" y2="${l.y.toFixed(1)}" class="ops-pline ops-${l.name}"/>`).join("");
  const grid = g.ticks.map((t) => `<line x1="0" x2="100" y1="${t.y.toFixed(1)}" y2="${t.y.toFixed(1)}" class="ops-grid"/>`).join("");
  const yLabels = g.ticks.map((t) => `<span style="top:${t.y.toFixed(1)}px">${esc(t.label)}</span>`).join("");
  const lineLabels = g.lines.map((l) => `<span class="ops-plabel" style="top:${l.labelY.toFixed(1)}px">${esc(l.name)} ${esc(fmtAge(l.v))}</span>`).join("");
  const title = (d) => {
    const t = state.tasks.get(d.task_id);
    return `${t ? t.title : d.task_id.slice(0, 8)} · ${fmtAge(d.cycle_s)} · ended ${utcStamp(d.ended_at)}`;
  };
  const dots = g.dots.map((d) => `<button type="button" class="ops-dot sm" style="left:${d.cx.toFixed(2)}%;top:${d.cy.toFixed(1)}px"
      data-task="${esc(d.task_id)}" data-run="${esc(d.run_id)}" title="${esc(title(d))}" aria-label="${esc(title(d))}"></button>`).join("");
  const table = tableBlock("Scatter as a table", ["Task", "Ended", "Cycle time"],
    g.dots.map((d) => { const t = state.tasks.get(d.task_id); return [esc(t ? t.title : d.task_id.slice(0, 8)), esc(utcStamp(d.ended_at)), esc(fmtAge(d.cycle_s))]; }));
  return `<div class="ops-plot-row">
      <div class="ops-yaxis">${yLabels}</div>
      <div class="ops-plot" style="height:200px">
        <svg viewBox="0 0 100 200" preserveAspectRatio="none" aria-hidden="true">${grid}${lines}</svg>
        ${lineLabels}${dots}
      </div>
    </div>
    <div class="ops-xaxis"><span style="left:0;transform:none">${esc(dayLabel(cur.window.from))}</span><span style="left:100%;transform:translateX(-100%)">now</span></div>
    <p class="dnote">One dot per run that ended done, by when it ended and how long it took; dashed lines at the window's p50 and p85.
      ${cur.finished_runs.length >= 2000 ? "Only the newest 2000 finished runs are drawn." : ""}</p>
    ${table}`;
}

function cfdHtml(cur) {
  const g = cfdGeometry(cur.days, { width: 100, height: 180 });
  if (g.empty) return `<div class="empty">Nothing moved through the line in this window.</div>`;
  const bands = g.bands.map((b) => `<polygon points="${b.points}" class="cfd-${b.id}"><title>${esc(b.label)}</title></polygon>`).join("");
  const legend = g.bands.map((b) => `<span><i class="cfd-${b.id}"></i>${esc(b.label)}</span>`).join("");
  const table = tableBlock("Cumulative flow as a table", ["Step ending", "Finished (cum.)", "In progress", "Waiting"],
    g.stacks.map((s) => [esc(utcStamp(s.to)), String(s.done), String(s.progress - s.done), String(s.waiting - s.progress)]));
  return `<div class="chleg">${legend}</div>
    <div class="ops-plot-row"><div class="ops-yaxis"><span style="top:6px">${g.max}</span><span style="top:174px">0</span></div>
      <div class="ops-plot" style="height:180px"><svg viewBox="0 0 100 180" preserveAspectRatio="none" role="img"
        aria-label="Cumulative flow: finished, in progress and waiting at the end of each day of the window">${bands}</svg></div></div>
    <div class="ops-xaxis"><span style="left:0;transform:none">${esc(dayLabel(cur.window.from))}</span><span style="left:100%;transform:translateX(-100%)">now</span></div>
    <p class="dnote">Finished counts from the window's start. Waiting only shows runs that record when they were queued; a task due
      and not yet dispatched has no run and is not in it.</p>
    ${table}`;
}

function healthHtml() {
  const seg = `<span class="seg" id="ops-window">${["7d", "30d"].map((w) => `<button type="button" data-w="${w}" class="${w === windowKey ? "on" : ""}">${w}</button>`).join("")}</span>`;
  const head = `<h3>Process health<span class="r">${esc(windowKey)} against the ${esc(windowKey)} before it</span>${seg}</h3>`;
  if (!health) {
    return `<section class="dcard ops-card">${head}<div class="err">${esc(healthError || "Health is not available right now.")}</div></section>`;
  }
  const cur = health.health.current;
  const prev = health.health.previous;
  const scopeNote = state.scope !== null
    ? `<p class="dnote">Health covers ${esc(scopeLabel())} itself, not the scopes nested under it -- a window's percentiles and rates
        cannot be added up across scopes, so the daemon computes them for exactly one.</p>`
    : "";
  const since = health.recorded_since
    ? `<p class="dnote">Queue waits and fail kinds are recorded from ${esc(utcStamp(health.recorded_since))} on; a figure over a window reaching
        back before that says so rather than reading the missing runs as zeros.</p>`
    : `<p class="dnote">No run records queue waits or fail kinds yet; those figures read "unknown", never zero.</p>`;
  const kinds = failKindRows(cur);
  const kindHtml = kinds.length
    ? kinds.map((k) => `<div class="stn"><span class="nm">${esc(k.label)}</span><span class="track"><i style="width:${k.w.toFixed(1)}%;background:var(--fault)"></i></span><span class="c">${k.n}</span></div>`).join("")
    : `<div class="empty">Nothing failed or was cancelled in this window.</div>`;
  const multTable = tableBlock("Health as a table", ["Figure", esc(windowKey), `previous ${esc(windowKey)}`],
    [...MULTIPLES, ...TILES].map((m) => [esc(m.title), esc(figureText(cur[m.fig], m.fmt)), esc(figureText(prev[m.fig], m.fmt))]));
  return `<section class="dcard ops-card">
    ${head}
    ${scopeNote}
    <div class="ops-mults">${MULTIPLES.map((m) => multipleHtml(m, cur, prev)).join("")}${TILES.map((t) => tileHtml(t, cur, prev)).join("")}</div>
    <div class="chleg"><span><i class="l-cur"></i>this ${esc(windowKey)}</span><span><i class="l-ghost"></i>the ${esc(windowKey)} before</span>
      <span>${cur.finished} finished · ${cur.interventions} intervention${cur.interventions === 1 ? "" : "s"} by a person</span></div>
    ${multTable}
    <div class="stngrp">Failed or cancelled, by why</div>
    ${kindHtml}
    <details class="ops-more" id="ops-scatter"${showScatter ? " open" : ""}><summary>Cycle-time scatter</summary>${showScatter ? scatterHtml(cur) : ""}</details>
    <details class="ops-more" id="ops-cfd"${showCfd ? " open" : ""}><summary>Cumulative flow</summary>${showCfd ? cfdHtml(cur) : ""}</details>
    ${since}
  </section>`;
}

// --------------------------------------------------------------- schedules

function schedulesHtml() {
  const rows = scheduleRows(report.schedules, inScope);
  const nodes = blockedWorkflowNodes([...state.tasks.values()], inScope);
  const body = rows.length ? `<div class="ops-tscroll"><table class="ops-sched">
      <thead><tr><th>Task</th><th>State</th><th>Schedule</th><th>Next</th><th>Last start</th><th></th></tr></thead>
      <tbody>${rows.map((r) => {
        const row = { ...r, key: `sched|${r.task_id}` };
        const late = r.last_late_s !== undefined && r.last_late_s !== null ? `${fmtAge(r.last_late_s)} after its slot` : "—";
        return `<tr>
          <td><a href="#" class="ops-open" data-task="${esc(r.task_id)}">${esc(r.title)}</a><div class="sub">${esc(r.scope)}</div></td>
          <td><span class="ops-state st-${esc(r.state)}">${esc(r.state)}</span>${r.skipped ? `<div class="sub">${r.skipped} slot${r.skipped === 1 ? "" : "s"} passed over</div>` : ""}${r.retrying ? `<div class="sub">a retry fires first</div>` : ""}</td>
          <td>${esc(r.schedule)}<div class="sub">${esc(r.zone)}</div></td>
          <td class="mono">${esc(r.next)}</td>
          <td class="sub">${esc(late)}</td>
          <td><div class="ops-acts">${actionButtons(row, scheduleActions(r))}</div></td>
        </tr>`;
      }).join("")}</tbody></table></div>`
    : `<div class="empty">No scheduled tasks${state.scope === null ? "" : ` in ${esc(scopeLabel())}`}.</div>`;
  const wf = nodes.length ? `<div class="stngrp">Blocked workflow nodes · holding back their run</div>
    ${nodes.map((t) => { const o = describeWorkflowOrigin(t.workflow_origin); return `<div class="ops-wf"><a href="#" class="ops-open" data-task="${esc(t.id)}">${esc(t.title)}</a>
      <span class="sub">${esc(t.scope)} · <a href="${esc(o.href)}">${esc(o.label)}</a></span></div>`; }).join("")}` : "";
  return `<section class="dcard ops-card">
    <h3>Schedules<span class="r">late, missed and paused first · times in UTC, the cron's own zone beneath</span></h3>
    ${body}
    ${wf}
  </section>`;
}

// ------------------------------------------------------------------ wiring

/// The slot a row's schedule is next due -- what "skip next" names, so the
/// daemon can refuse it if the schedule moved on. The report's schedule row
/// first, as that is what the page drew; the task list otherwise.
function nextSlotOf(taskId) {
  const row = ((report && report.schedules) || []).find((s) => s.task_id === taskId);
  if (row && row.next_run_at) return row.next_run_at;
  const t = state.tasks.get(taskId);
  return t ? t.next_run_at : undefined;
}

function wire(host, rows) {
  const byKey = new Map(rows.map((r) => [r.key, { ...r, next_run_at: r.task_id ? nextSlotOf(r.task_id) : undefined }]));
  for (const r of scheduleRows(report.schedules, inScope)) byKey.set(`sched|${r.task_id}`, { ...r, key: `sched|${r.task_id}` });

  for (const el of host.querySelectorAll(".ops-row")) {
    const go = () => {
      if (el.dataset.task) openTask(el.dataset.task, el.dataset.run);
      else if (el.dataset.href) location.hash = el.dataset.href;
    };
    el.onclick = (e) => { if (!e.target.closest("button, a")) go(); };
    el.onkeydown = (e) => { if ((e.key === "Enter" || e.key === " ") && e.target === el) { e.preventDefault(); go(); } };
  }
  for (const b of host.querySelectorAll(".ops-act")) {
    b.onclick = (e) => {
      e.stopPropagation();
      const row = byKey.get(b.dataset.key);
      if (row) openActionDialog(b.dataset.act, row);
    };
  }
  for (const b of host.querySelectorAll(".ops-bulk")) {
    b.onclick = () => openBulkDialog(bulkPreview(rows, b.dataset.bulk));
  }
  for (const b of host.querySelectorAll(".ops-dot")) {
    b.onclick = () => openTask(b.dataset.task, b.dataset.run);
  }
  for (const a of host.querySelectorAll("a.ops-open")) {
    a.onclick = (e) => { e.preventDefault(); openTask(a.dataset.task); };
  }
  const seg = $("ops-window");
  if (seg) {
    for (const b of seg.querySelectorAll("button")) {
      b.onclick = () => {
        if (b.dataset.w === windowKey) return;
        windowKey = b.dataset.w;
        loadOperations();
      };
    }
  }
  // Drawn only once opened: the scatter can be two thousand buttons, and
  // most visits never look at it.
  const scatter = $("ops-scatter");
  if (scatter) scatter.ontoggle = () => { if (scatter.open !== showScatter) { showScatter = scatter.open; renderOperations(); } };
  const cfd = $("ops-cfd");
  if (cfd) cfd.ontoggle = () => { if (cfd.open !== showCfd) { showCfd = cfd.open; renderOperations(); } };
}

// ----------------------------------------------------------------- actions

/// One action, confirmed. The reason is optional except for an answer; the
/// daemon journals it with who asked. A refusal -- a run that is no longer
/// blocked, a skip while a retry is pending, a task already running --
/// comes back as the daemon's own sentence and stays in the dialog, so the
/// person sees why rather than a dialog that simply closed.
export function openActionDialog(action, row) {
  dropModal();
  const needsText = action === "answer";
  const reasonNote = needsText
    ? "required -- journaled as an intervention, with who answered"
    : "optional -- journaled with who asked";
  scrim(`
    <header><div><h2>${esc(actionLabel(action))}</h2><code class="id">${esc(row.title || row.task_id || "")}</code></div>
      <button class="x" id="oa-close" aria-label="Close">&times;</button></header>
    <div class="body">
      <p class="env-note">${esc(actionConsequence(action, row))}</p>
      ${row.reason && needsText ? `<label>What it said</label><pre>${esc(row.reason)}</pre>` : ""}
      ${needsText ? `<label for="oa-text">Answer <span class="sub" style="text-transform:none">typed into the run's session and never recorded</span></label>
        <textarea id="oa-text" rows="3"></textarea>` : ""}
      <label for="oa-reason">Reason <span class="sub" style="text-transform:none">${esc(reasonNote)}</span></label>
      <textarea id="oa-reason" rows="2"></textarea>
      <div class="err" id="oa-err" role="alert"></div>
      <div class="row-btns" style="margin-top:16px">
        <button class="btn ${action === "cancel" ? "danger" : "primary"}" id="oa-confirm">${esc(actionLabel(action))}</button>
        <button class="btn" id="oa-cancel">Back</button>
      </div>
    </div>`);
  const values = () => ({ reason: $("oa-reason").value, text: needsText ? $("oa-text").value : "" });
  const go = $("oa-confirm");
  const sync = () => { go.disabled = !actionReady(action, values()); };
  for (const id of ["oa-reason", "oa-text"]) { const el = $(id); if (el) el.oninput = sync; }
  sync();
  $("oa-close").onclick = closeModal;
  $("oa-cancel").onclick = closeModal;
  (needsText ? $("oa-text") : $("oa-reason")).focus();
  go.onclick = async () => {
    const req = actionRequest(action, row, values());
    if (!req) return;
    $("oa-err").textContent = "";
    go.disabled = true;
    try {
      await api(req.path, { method: req.method, body: JSON.stringify(req.body) });
      closeModal();
      loadOperations();
    } catch (e) {
      $("oa-err").textContent = e.message;
      sync();
    }
  };
}

/// Bulk run again or cancel: a preview of every row it would touch, with
/// the count, before anything is sent; then one request per row, in turn,
/// each row saying how it went -- a refusal of one never hides the rest.
function openBulkDialog(preview) {
  dropModal();
  const label = actionLabel(preview.action);
  scrim(`
    <header><div><h2>${esc(label)} · ${preview.count} task${preview.count === 1 ? "" : "s"}</h2>
      <span class="sub">every row below, and nothing else</span></div>
      <button class="x" id="ob-close" aria-label="Close">&times;</button></header>
    <div class="body">
      <p class="env-note">${esc(preview.action === "run_again"
        ? `Starts a new attempt of each of these ${preview.count} tasks. Each spends a model call.`
        : `Cancels the running attempt of each of these ${preview.count} tasks. Each counts as scrap.`)}</p>
      <div class="ops-tscroll"><table><thead><tr><th>Task</th><th>Scope</th><th>Why it is here</th><th>Outcome</th></tr></thead>
        <tbody>${preview.rows.map((r, i) => `<tr><td>${esc(r.title || r.task_id)}</td><td class="sub">${esc(r.scope || "")}</td>
          <td class="sub">${esc(kindLabel(r.kind))} · ${esc(fmtAge(r.age))}</td><td class="sub" id="ob-out-${i}">—</td></tr>`).join("")}</tbody></table></div>
      <label for="ob-reason">Reason <span class="sub" style="text-transform:none">optional -- journaled on each task</span></label>
      <textarea id="ob-reason" rows="2"></textarea>
      <div class="err" id="ob-err" role="alert"></div>
      <div class="row-btns" style="margin-top:16px">
        <button class="btn ${preview.action === "cancel" ? "danger" : "primary"}" id="ob-confirm">${esc(label)} ${preview.count}</button>
        <button class="btn" id="ob-cancel">Back</button>
      </div>
    </div>`);
  $("ob-close").onclick = closeModal;
  $("ob-cancel").onclick = closeModal;
  $("ob-confirm").onclick = async () => {
    const button = $("ob-confirm");
    button.disabled = true;
    const reason = $("ob-reason").value;
    let failed = 0;
    for (const [i, row] of preview.rows.entries()) {
      const req = actionRequest(preview.action, row, { reason });
      const out = $(`ob-out-${i}`);
      try {
        await api(req.path, { method: req.method, body: JSON.stringify(req.body) });
        if (out) out.textContent = "sent";
      } catch (e) {
        failed += 1;
        if (out) { out.textContent = e.message; out.classList.add("bad"); }
      }
    }
    $("ob-err").textContent = failed ? `${failed} of ${preview.count} refused -- the reason is on each row.` : "";
    button.hidden = true;
    $("ob-cancel").textContent = "Close";
    loadOperations();
  };
}

export function wireOperations() {
  const refresh = $("ops-refresh");
  if (refresh) refresh.onclick = () => loadOperations();
  // Leaving the page is also leaving the tab.
  addEventListener("pagehide", () => { if (visible) rememberSeen(); });
}
