//! The L6 Direction, Scenarios tab (`#100`): play out a what-if -- a goal
//! changes, a regulation tightens, capacity drops -- against the same policy
//! evaluator, metric registry and goals catalogue Policy and Goals
//! themselves read. `scenarios-model.js` holds every pure shaping function
//! this file draws from; this file only fetches, renders and wires clicks --
//! the same split `policy.js`/`goals.js` keep.
//!
//! Rail semantics: nothing selected fetches the whole instance
//! (`GET /api/scenarios`); a scope selected narrows to that scope and its
//! descendants (`GET /api/scenarios?scope=`) -- the daemon does the
//! narrowing itself, the same pattern `policy.js`'s `loadPolicy` follows.
//! `POST /api/scenarios/whatif` carries the selected scope too (#164), so
//! measured baselines, outcomes and the slider panel share one subtree.

import { $, api, esc, state } from "./core.js";
import { scrim, closeModal, dropModal } from "./modal.js";
import { refLinks } from "./policy-model.js";
import {
  DRIVER_DEFS,
  THROUGHPUT_HISTORY_WEEKS,
  complianceSummary,
  completionSummary,
  controlReason,
  deltaTable,
  deltaTone,
  driverRange,
  driverUnavailableReason,
  fanGeometry,
  formatWeek,
  goalProbabilityLabel,
  krLabel,
  matrixCells,
  metricTitle,
  narrativeBoard,
  policyTabHref,
  promoteBody,
  signpostRows,
  signpostStrip,
  tornadoLayout,
  whatifBody,
} from "./scenarios-model.js";

// ------------------------------------------------------------------- state
//
// Kept module-private rather than on `state` (`core.js`): nothing else in
// the app reads a scenario, a slider position or which board segment is
// showing, and keeping it here means an edit to this file never touches the
// shared state object another tab's own PR is also changing.

let report = null;
let reportError = null;
let asked = 0;
let metricDefs = {}; // metric id -> MetricDefView, for the ids baseline.metrics names

let mode = "board"; // "board" | "workshop"

let driverScenario = null; // null = baseline
let moved = {}; // driver id -> the value a slider was last dragged to
let whatifResult = null;
let whatifError = null;
let whatifAsked = 0;
let whatifTimer = null;

let matrixScenario = null;

// -------------------------------------------------------------- fan charts

/// A fixed-height placeholder standing in for the chart, so a card whose
/// forecast has nothing to draw does not shrink and shuffle the layout
/// around it -- `.scn-fan-empty` is sized to roughly what `.scn-fan` itself
/// occupies.
function fanEmptyState(text) {
  return `<div class="scn-fan-empty">${esc(text)}</div>`;
}

/// The y-axis column (top = `yMax`, bottom = 0, "in items") -- a flex sibling
/// of `.scn-fan-plot`, not text drawn inside the SVG: that viewBox is
/// stretched non-uniformly by `preserveAspectRatio="none"` (600×160 squashed
/// to whatever width a card gives it), which would distort glyphs along
/// with the geometry. Plain HTML beside the SVG scales with the card
/// without any of that.
function fanYAxisHtml(geo) {
  const yMaxLabel = Number.isInteger(geo.yMax) ? String(geo.yMax) : geo.yMax.toFixed(1);
  return `<div class="scn-fan-yaxis"><span>${esc(yMaxLabel)} items</span><span>0</span></div>`;
}

/// The x-axis row ("now" … "week N") and the p50/p90 completion-marker
/// callouts -- percentage-positioned against `.scn-fan-plot`/`.scn-fan-xaxis`,
/// which are exactly as wide as `.scn-fan` itself (both plain flex children
/// of the same row the y-axis occupies, so a percentage of one is a
/// percentage of the other -- see the CSS's own comment on why this is a
/// flex row rather than an absolutely-positioned overlay).
function fanXAxisHtml(geo) {
  const pct = (px) => (geo.width ? (px / geo.width) * 100 : 0);
  const xTicks = geo.xTicks
    .map((t, i) => {
      // The first tick left-aligns, the last right-aligns, anything between
      // centres on its own point -- so "now" and the final week both stay
      // inside the chart's own width instead of hanging off the edge.
      const align = i === 0 ? "left" : i === geo.xTicks.length - 1 ? "right" : "center";
      return `<span class="scn-fan-xtick scn-fan-xtick-${align}" style="left:${pct(t.x).toFixed(1)}%">${esc(t.label)}</span>`;
    })
    .join("");
  return `<div class="scn-fan-xaxis-row"><div class="scn-fan-xaxis-spacer"></div><div class="scn-fan-xaxis">${xTicks}</div></div>`;
}

function fanMarkerLabelsHtml(geo) {
  const pct = (px) => (geo.width ? (px / geo.width) * 100 : 0);
  const label = (marker, cls, text) => (marker ? `<span class="scn-fan-mlabel ${cls}" style="left:${pct(marker.x).toFixed(1)}%">${esc(text)}</span>` : "");
  return `${label(geo.p50Marker, "scn-fan-mlabel-p50", "p50")}${label(geo.p90Marker, "scn-fan-mlabel-p90", "p90")}`;
}

function fanChartSvg(geo, label) {
  if (geo.empty) return fanEmptyState(geo.reason);
  if (geo.clearedAlready) return fanEmptyState("Nothing in the backlog to clear.");
  if (geo.noSignal) return fanEmptyState(`No finished runs in the last ${THROUGHPUT_HISTORY_WEEKS} weeks — nothing to forecast from yet.`);
  // A vertical line the full height of the chart, not a dot: under
  // `preserveAspectRatio="none"` the viewBox (600×160) is squashed to
  // whatever a card's own width gives it, so a small-radius circle can
  // shrink to a near-invisible sliver on a narrow card. A full-height line
  // also reads more honestly as "this week", on a chart whose x axis is
  // weeks.
  const markers = [];
  if (geo.p50Marker) {
    markers.push(
      `<line class="scn-fan-marker scn-fan-marker-p50" x1="${geo.p50Marker.x.toFixed(1)}" x2="${geo.p50Marker.x.toFixed(1)}" y1="0" y2="${geo.height}"><title>p50 completion: week ${geo.p50Marker.week}</title></line>`,
    );
  }
  if (geo.p90Marker) {
    markers.push(
      `<line class="scn-fan-marker scn-fan-marker-p90" x1="${geo.p90Marker.x.toFixed(1)}" x2="${geo.p90Marker.x.toFixed(1)}" y1="0" y2="${geo.height}"><title>p90 completion: week ${geo.p90Marker.week}</title></line>`,
    );
  }
  const backlogLine =
    geo.backlogY !== null
      ? `<line class="scn-fan-backlog" x1="0" y1="${geo.backlogY.toFixed(1)}" x2="${geo.width}" y2="${geo.backlogY.toFixed(1)}"><title>backlog</title></line>`
      : "";
  return `<div class="scn-fan-row">
    ${fanYAxisHtml(geo)}
    <div class="scn-fan-plot">
      <svg class="scn-fan" viewBox="0 0 ${geo.width} ${geo.height}" preserveAspectRatio="none" role="img"
          aria-label="Forecast fan chart for ${esc(label)}: p10 to p90 band with the p50 line, never a single line.">
        <polygon class="scn-fan-band" points="${geo.bandPoints}"/>
        <polyline class="scn-fan-median" points="${geo.medianPoints}" fill="none"/>
        ${backlogLine}
        ${markers.join("")}
      </svg>
      ${fanMarkerLabelsHtml(geo)}
    </div>
  </div>
  ${fanXAxisHtml(geo)}
  <div class="chleg"><span><i class="scn-leg-band"></i>p10–p90</span><span><i class="scn-leg-median"></i>p50</span>${
    geo.backlogY !== null ? `<span><i class="scn-leg-backlog"></i>backlog</span>` : ""
  }</div>`;
}

// ---------------------------------------------------------------- signposts

function signpostStripHtml() {
  if (!report) return "";
  const rows = signpostStrip(report);
  if (!rows.length) return `<div class="sub">No signposts declared.</div>`;
  return rows
    .map(
      (r) => `<div class="scn-signpost">
    <span class="scn-light ${r.cls}" aria-hidden="true"></span>
    <div class="scn-signpost-text">
      <div class="scn-signpost-title">${esc(metricTitle(r.metric, metricDefs))} <span class="sub">${esc(r.threshold)}</span></div>
      <div class="sub">${esc(r.scenarioTitle)} · ${esc(r.label)} — ${esc(r.reason)}</div>
    </div>
  </div>`,
    )
    .join("");
}

// -------------------------------------------------------------------- board

function baselineCardHtml() {
  const geo = fanGeometry(report.baseline.forecast, {});
  const fc = report.baseline.forecast.completion_week;
  return `<article class="scn-card scn-card-baseline">
    <div class="scn-card-head"><h3>Baseline</h3><span class="sub">today, no overlay</span></div>
    ${fanChartSvg(geo, "the baseline")}
    <div class="sub">completion, from the p10–p90 band: ${esc(completionSummary(fc))}</div>
  </article>`;
}

/// A scenario's own `drivers:` overrides, exactly as authored -- shown even
/// though `evaluate_outcomes`/`apply_overrides` silently drop a *relative*
/// override (`×N`, `+N%`, `+N`) for a driver the shared baseline carries no
/// value for at all (a fresh instance's `first_pass_yield`/`scrap_rate`,
/// with no finished runs yet to compute one from -- `apply_overrides`'s own
/// doc comment). No `Finding` says so either (deviation worth raising with
/// scenarios-daemon), so this is the one place a person can see that an
/// override they authored is quietly doing nothing: `scn-chip-dropped` marks
/// exactly that case, from `s.drivers.overridden` missing the id.
function driverOverrideChipsHtml(s) {
  const entries = Object.entries(s.scenario.drivers || {});
  if (!entries.length) return "";
  const applied = s.drivers.overridden || {};
  return `<div class="scn-chips">${entries
    .map(([id, raw]) => {
      const isText = typeof raw === "string";
      const text = isText ? raw : `${raw} — unquoted, a finding`;
      const dropped = isText && !(id in applied);
      const hint = dropped ? `No baseline value for ${id} here -- this override is silently ignored; use =N to set it outright.` : "";
      // The full reason lives in `title`, for a hover; a short version is
      // inline too (build item 7) -- a struck-through chip alone does not
      // say *why* it has no effect, and not everyone reading this hovers.
      const inline = dropped ? ` <span class="scn-chip-reason">(no baseline yet)</span>` : "";
      return `<span class="scn-chip${dropped ? " scn-chip-dropped" : ""}"${hint ? ` title="${esc(hint)}"` : ""}>${esc(id)}: ${esc(text)}${inline}</span>`;
    })
    .join("")}</div>`;
}

function scenarioCardHtml(s) {
  const geo = fanGeometry(s.forecast, { backlog: s.backlog.total });
  const fc = s.forecast.completion_week;
  const kindChips = (s.scenario.kind || []).map((k) => `<span class="scn-chip">${esc(k)}</span>`).join("");
  const signposts = signpostRows(s);
  // A short label next to each light -- metric title + state -- not a bare
  // dot: a tooltip/title alone is a mouse-only affordance, and a card is
  // meant to be scanned without hovering every one of them.
  const lights = signposts
    .map((sp) => {
      const title = metricTitle(sp.metric, metricDefs);
      const full = `${title} ${sp.threshold} — ${sp.label}: ${sp.reason}`;
      // The state class rides the dot (`.scn-light`), the same element it
      // decorates in the signpost strip -- so the strip's own colour rules
      // (`.scn-sp-triggered` etc.) apply unchanged; the chip around it is a
      // plain, neutral pill, with the state name already spelled out in the
      // visible text.
      return `<span class="scn-signpost-chip" title="${esc(full)}"><i class="scn-light ${sp.cls}" aria-hidden="true"></i>${esc(title)}: ${esc(sp.label)}</span>`;
    })
    .join("");
  const goalsChips = (s.goals || [])
    .map((g) => `<span class="scn-chip">${esc(krLabel(g.kr))}: ${esc(goalProbabilityLabel(g.probability))}</span>`)
    .join("");
  return `<article class="scn-card" data-scenario="${esc(s.scenario.name)}">
    <div class="scn-card-head"><h3>${esc(s.scenario.title)}</h3></div>
    <code class="id">${esc(s.scenario.name)}</code>
    <div class="scn-chips">${kindChips}</div>
    ${s.scenario.assumptions ? `<details class="scn-assumptions"><summary>Assumptions</summary><p>${esc(s.scenario.assumptions.trim())}</p></details>` : ""}
    ${driverOverrideChipsHtml(s)}
    ${fanChartSvg(geo, s.scenario.title)}
    <div class="sub">completion: ${esc(completionSummary(fc, s.backlog.total))} · backlog ${esc(String(s.backlog.total))}</div>
    <div class="sub">${esc(complianceSummary(s))}</div>
    ${goalsChips ? `<div class="scn-chips">${goalsChips}</div>` : ""}
    ${lights ? `<div class="scn-card-signposts">${lights}</div>` : ""}
    <div class="row-btns" style="margin-top:8px">
      <button type="button" class="btn" data-select-driver="${esc(s.scenario.name)}">Drivers</button>
      <button type="button" class="btn primary" data-promote="${esc(s.scenario.name)}">Promote…</button>
    </div>
  </article>`;
}

function emptyStateHtml() {
  return `<div class="empty">No scenarios yet. Add a file under
    <code class="id">.factory/scenarios/&lt;name&gt;.yaml</code>, for example:
    <pre class="scn-example">name: capacity-drop
title: Capacity drops 20% next quarter
kind: [drivers]
horizon: 13w
drivers:
  capacity_factor: "-20%"</pre>
    See README.md, "Scenarios", for the full field list.</div>`;
}

function wireBoardButtons(root) {
  for (const b of root.querySelectorAll("[data-select-driver]")) {
    b.onclick = () => {
      driverScenario = b.dataset.selectDriver;
      moved = {};
      whatifResult = null;
      whatifError = null;
      renderDriverPanel();
      doWhatif();
      $("scn-drivers")?.scrollIntoView({ block: "start" });
    };
  }
  for (const b of root.querySelectorAll("[data-promote]")) {
    b.onclick = () => openPromoteModal(b.dataset.promote);
  }
}

// -------------------------------------------------------------- delta table

/// `tone: "unknown"` only means no colour direction could be drawn (`deltaTone`'s
/// own restraint against guessing a direction from one missing side) -- it
/// never means the value itself is unknown. `null`/`undefined` on one side
/// alone (a framework `newly_open` introduces, a forecast that never clears
/// within the horizon) still shows: only both sides missing is a dash.
function formatDeltaCell(c, unit) {
  if (c.before === null && c.after === null) return "—";
  const fmt = (v) => {
    // `week` first: `formatWeek`'s own `null` reading ("never within the
    // horizon") is a real fact about a forecast, not a missing value the
    // way an absent framework rollup is -- a dash there would say less than
    // the daemon actually answered.
    if (unit === "week") return formatWeek(v);
    if (v === null || v === undefined) return "—";
    return Number.isInteger(v) ? String(v) : v.toFixed(2);
  };
  return `${fmt(c.before)} → ${fmt(c.after)}`;
}

function deltaTableHtml() {
  const t = deltaTable(report);
  if (!t.rows.length) return `<div class="empty">Nothing to compare yet.</div>`;
  const head = `<tr><th>Metric / outcome</th>${t.scenarios.map((n) => `<th>${esc(n)}</th>`).join("")}</tr>`;
  const rows = t.rows
    .map(
      (r) => `<tr>
    <td>${esc(r.label)} <span class="scn-band-tag">${r.exact ? "exact" : "p10–p90 band"}</span></td>
    ${r.cells.map((c) => `<td class="scn-delta scn-tone-${c.tone} scn-mag-${c.mag}" title="${esc(c.reason || "")}">${formatDeltaCell(c, r.unit)}</td>`).join("")}
  </tr>`,
    )
    .join("");
  return `<div class="scn-table"><table class="scn-delta-table"><thead>${head}</thead><tbody>${rows}</tbody></table></div>`;
}

// ------------------------------------------------------------- driver panel

function neutralDefault(id) {
  return id === "capacity_factor" ? 1 : 0;
}

function currentDriverValues() {
  if (whatifResult) return whatifResult.drivers.overridden;
  if (driverScenario) {
    const s = (report.scenarios || []).find((x) => x.scenario.name === driverScenario);
    if (s) return s.drivers.overridden;
  }
  return report.baseline.drivers;
}

function driverRowHtml(def, values) {
  if (def.unavailable || (def.measuredCost && !Number.isFinite(report.baseline.drivers[def.id]))) {
    return `<div class="scn-driver-row scn-driver-disabled">
      <div class="scn-driver-label">${esc(def.title)} <span class="scn-chip scn-assumption">unavailable</span></div>
      <div class="sub">${esc(driverUnavailableReason(def, report.baseline.metrics, metricDefs))}</div>
    </div>`;
  }
  const raw = moved[def.id] !== undefined ? moved[def.id] : values[def.id] !== undefined ? values[def.id] : neutralDefault(def.id);
  const range = driverRange(def.id, report.baseline.drivers[def.id]);
  range.max = Math.max(range.max, raw);
  return `<div class="scn-driver-row">
    <div class="scn-driver-label">${esc(def.title)} ${def.assumption ? `<span class="scn-chip scn-assumption">assumption</span>` : ""}</div>
    <input type="range" id="scn-slider-${esc(def.id)}" min="${range.min}" max="${range.max}" step="${range.step}" value="${raw}" data-driver="${esc(def.id)}" aria-label="${esc(def.title)}">
    <output id="scn-slider-out-${esc(def.id)}" for="scn-slider-${esc(def.id)}">${Number(raw).toFixed(2)}</output>
  </div>`;
}

function outcomeHtml() {
  if (whatifError) return `<div class="err">${esc(whatifError)}</div>`;
  if (!whatifResult) return `<div class="sub">Move a slider to see outcomes.</div>`;
  const before = whatifResult.drivers.outcomes_before.effective_throughput;
  const after = whatifResult.drivers.outcomes_after.effective_throughput;
  if (!Number.isFinite(before) || !Number.isFinite(after)) {
    return `<div class="empty">${esc(whatifResult.drivers.outcome_reasons_after?.effective_throughput || "Effective throughput is unavailable")}</div>`;
  }
  const costs = [["weekly_cost", "Weekly USD", "$"], ["weekly_tokens", "Weekly tokens", ""]].map(([id, title, prefix]) => {
    const a = whatifResult.drivers.outcomes_before[id];
    const b = whatifResult.drivers.outcomes_after[id];
    if (!Number.isFinite(a) || !Number.isFinite(b)) {
      const reason = whatifResult.drivers.outcome_reasons_after?.[id] || whatifResult.drivers.outcome_reasons_before?.[id] || "complete measured usage is unavailable";
      return `<div class="sub">${title}: unavailable — ${esc(reason)}</div>`;
    }
    return `<div class="scn-outcome">${title}: ${prefix}${a.toFixed(2)} → ${prefix}${b.toFixed(2)}</div>`;
  }).join("");
  const tone = deltaTone(before, after, "higher");
  const fc = whatifResult.forecast.completion_week;
  return `<div class="scn-outcome scn-tone-${tone.tone}">Effective throughput: ${before.toFixed(2)} → ${after.toFixed(2)} / week</div>
    ${before === 0 && after === 0 ? '<div class="empty">Effective throughput is 0 -- move "Throughput per week" above to explore a hypothetical rate.</div>' : ""}
    ${costs}
    <div class="sub">completion: p50 ${esc(formatWeek(fc.p50))} · p90 ${esc(formatWeek(fc.p90))}</div>`;
}

let tornadoOutcome = "effective_throughput";

function tornadoSvg() {
  if (!whatifResult) return "";
  const barsForOutcome = whatifResult.drivers.tornados?.[tornadoOutcome] || (tornadoOutcome === "effective_throughput" ? whatifResult.drivers.tornado : []);
  const layout = tornadoLayout(barsForOutcome);
  if (!layout.bars.length) return `<div class="empty">No drivers to compare.</div>`;
  // Every bar spans zero exactly when `effective_throughput` itself is zero
  // on both sides (`outcomeHtml`'s own note covers why) -- three empty
  // hairlines say nothing a person can act on; one line does.
  if (layout.bars.every((b) => b.span === 0)) {
    return `<div class="empty">No driver moves ${esc(tornadoOutcome)} right now -- see the note above.</div>`;
  }
  const rowH = 24;
  const gap = 8;
  const labelW = 130;
  const chartW = layout.width;
  const height = layout.bars.length * (rowH + gap);
  const bars = layout.bars
    .map((b, i) => {
      const y = i * (rowH + gap);
      const w = b.hairline ? 2 : b.width;
      const x = b.hairline ? b.x - 1 : b.x;
      return `<g>
      <text class="scn-tornado-label" x="0" y="${y + rowH / 2 + 4}">${esc(b.driver)}</text>
      <rect class="scn-tornado-bar${b.hairline ? " scn-tornado-hairline" : ""}" x="${(labelW + x).toFixed(1)}" y="${y}" width="${Math.max(w, 0.5).toFixed(1)}" height="${rowH}"><title>${esc(b.driver)}: ${b.low.toFixed(2)} .. ${b.high.toFixed(2)} (span ${b.span.toFixed(2)})</title></rect>
    </g>`;
    })
    .join("");
  return `<svg class="scn-tornado" viewBox="0 0 ${labelW + chartW} ${height}" role="img"
      aria-label="Tornado chart: each driver's swing on ${esc(tornadoOutcome)} at plus or minus 20 percent.">${bars}</svg>`;
}

function driverPanelHtml() {
  if (!report) return "";
  const values = currentDriverValues();
  const scopeCaption =
    state.scope !== null
      ? `<p class="env-note">Selected scope and descendants: <code class="id">${esc(state.scope)}</code>. Cost and token baselines use fully measured finished runs.</p>`
      : "";
  const options =
    `<option value="" ${!driverScenario ? "selected" : ""}>Baseline</option>` +
    (report.scenarios || [])
      .map((s) => `<option value="${esc(s.scenario.name)}" ${driverScenario === s.scenario.name ? "selected" : ""}>${esc(s.scenario.title)}</option>`)
      .join("");
  const sliders = DRIVER_DEFS.map((def) => driverRowHtml(def, values)).join("");
  const tornadoOptions = [["effective_throughput", "Effective throughput"], ["weekly_cost", "Weekly USD"], ["weekly_tokens", "Weekly tokens"]]
    .map(([id, title]) => `<option value="${id}" ${tornadoOutcome === id ? "selected" : ""}>${title}</option>`).join("");
  return `
    <div class="bar"><h3>Driver panel</h3><span class="sp"></span>
      <select id="scn-driver-select">${options}</select>
      <button class="btn" id="scn-driver-reset">Reset</button>
    </div>
    ${scopeCaption}
    <div class="scn-driver-grid">${sliders}</div>
    <h4 class="pol-sub-head">Outcomes</h4>
    <div id="scn-outcomes">${outcomeHtml()}</div>
    <h4 class="pol-sub-head">Tornado — ±20% per driver</h4>
    <select id="scn-tornado-outcome" aria-label="Tornado outcome">${tornadoOptions}</select>
    <div id="scn-tornado">${tornadoSvg()}</div>`;
}

function renderDriverPanel() {
  const el = $("scn-drivers");
  if (!el) return;
  el.innerHTML = driverPanelHtml();
  wireDriverPanel(el);
}

function wireDriverPanel(root) {
  root = root || $("scn-drivers");
  if (!root) return;
  const outcome = $("scn-tornado-outcome");
  if (outcome) outcome.onchange = () => {
    tornadoOutcome = outcome.value;
    const chart = $("scn-tornado");
    if (chart) chart.innerHTML = tornadoSvg();
  };
  const select = $("scn-driver-select");
  if (select) {
    select.onchange = () => {
      driverScenario = select.value || null;
      moved = {};
      whatifResult = null;
      whatifError = null;
      renderDriverPanel();
      void doWhatif();
    };
  }
  const reset = $("scn-driver-reset");
  if (reset) {
    reset.onclick = () => {
      moved = {};
      whatifResult = null;
      whatifError = null;
      renderDriverPanel();
      void doWhatif();
    };
  }
  for (const input of root.querySelectorAll("input[type=range]")) {
    input.oninput = () => {
      const id = input.dataset.driver;
      moved[id] = Number(input.value);
      const out = $(`scn-slider-out-${id}`);
      if (out) out.textContent = Number(input.value).toFixed(2);
      scheduleWhatif();
    };
  }
}

function scheduleWhatif() {
  if (whatifTimer) clearTimeout(whatifTimer);
  whatifTimer = setTimeout(doWhatif, 250);
}

/// Recompute driver outcomes, tornado and forecast server-side
/// (`POST /api/scenarios/whatif`), pure and read-only -- called once when a
/// scenario is selected in the driver panel (with no overrides yet, to seed
/// its own baseline outcomes/tornado, since `ScenarioBaseline` itself
/// carries no outcomes or tornado of its own) and again, debounced, on every
/// slider move. Guarded by `whatifAsked` the same way `policy.js`'s
/// `controlAsked` guards a control-detail fetch: a quick drag can fire
/// several requests, and only the newest answer may land.
async function doWhatif() {
  if (!report) return;
  const mine = ++whatifAsked;
  const body = whatifBody(driverScenario, moved, state.scope);
  try {
    const answer = await api("/api/scenarios/whatif", { method: "POST", body: JSON.stringify(body) });
    if (mine !== whatifAsked) return;
    whatifResult = answer.result;
    whatifError = null;
  } catch (e) {
    if (mine !== whatifAsked) return;
    whatifError = e.message;
  }
  const outcomesEl = $("scn-outcomes");
  if (outcomesEl) outcomesEl.innerHTML = outcomeHtml();
  const tornadoEl = $("scn-tornado");
  if (tornadoEl) tornadoEl.innerHTML = tornadoSvg();
}

// ---------------------------------------------------------- policy matrix

function matrixTableHtml(result) {
  const m = matrixCells(result);
  if (!m.scopes.length || !m.frameworks.length) {
    return `<div class="empty">This scenario carries no policy overlay, so there is nothing to compare.</div>`;
  }
  const head = `<tr><th>Scope</th>${m.frameworks.map((fw) => `<th>${esc(fw)}</th>`).join("")}</tr>`;
  const rows = m.scopes
    .map((scope) => {
      const cells = m.frameworks
        .map((fw) => {
          const cell = (m.cells[scope] || {})[fw];
          if (!cell || (!cell.newly_open.length && !cell.newly_stale.length && !cell.covered.length)) {
            return `<td class="sub">—</td>`;
          }
          const parts = [];
          if (cell.newly_open.length) parts.push(`${cell.newly_open.length} open`);
          if (cell.newly_stale.length) parts.push(`${cell.newly_stale.length} stale`);
          if (cell.covered.length) parts.push(`${cell.covered.length} covered`);
          return `<td><button type="button" class="linklike" data-matrix-scope="${esc(scope)}" data-matrix-fw="${esc(fw)}">${esc(parts.join(", "))}</button></td>`;
        })
        .join("");
      return `<tr><td>${esc(scope)}</td>${cells}</tr>`;
    })
    .join("");
  return `<div class="scn-table"><table class="scn-matrix-table"><thead>${head}</thead><tbody>${rows}</tbody></table></div>`;
}

function matrixSectionHtml() {
  const scenarios = report.scenarios || [];
  if (!scenarios.length) return "";
  if (!matrixScenario || !scenarios.some((s) => s.scenario.name === matrixScenario)) matrixScenario = scenarios[0].scenario.name;
  const options = scenarios
    .map((s) => `<option value="${esc(s.scenario.name)}" ${s.scenario.name === matrixScenario ? "selected" : ""}>${esc(s.scenario.title)}</option>`)
    .join("");
  const result = scenarios.find((s) => s.scenario.name === matrixScenario);
  return `
    <div class="bar"><h3>Policy what-if matrix</h3><span class="sp"></span><select id="scn-matrix-select">${options}</select></div>
    ${matrixTableHtml(result)}`;
}

function openMatrixCell(result, scope, fw) {
  dropModal();
  const m = matrixCells(result);
  const cell = (m.cells[scope] || {})[fw] || { newly_open: [], newly_stale: [], covered: [] };
  const rows = [
    ...cell.newly_open.map((ref) => ({ ref, kind: "newly_open", label: "newly open" })),
    ...cell.newly_stale.map((ref) => ({ ref, kind: "newly_stale", label: "newly stale" })),
    ...cell.covered.map((ref) => ({ ref, kind: "covered", label: "already covered" })),
  ];
  scrim(`
    <header><div><h2>${esc(fw)} in ${esc(scope)}</h2><span class="sub">${esc(result.scenario.title)}</span></div>
      <button class="x" id="mx-close">&times;</button></header>
    <div class="body">
      <table><thead><tr><th>Control</th><th>Change</th><th>Reason</th></tr></thead>
      <tbody>${rows
        .map(
          (r) =>
            `<tr><td><code class="id">${esc(r.ref)}</code></td><td>${esc(r.label)}</td><td class="sub">${esc(controlReason(result.scenario, r.ref, r.kind))}</td></tr>`,
        )
        .join("")}</tbody></table>
      <div class="row-btns" style="margin-top:12px"><a class="btn" href="${esc(policyTabHref(scope))}">Open Policy tab →</a></div>
    </div>`);
  $("mx-close").onclick = closeModal;
}

function wireMatrix(root) {
  const select = $("scn-matrix-select");
  if (select) {
    select.onchange = () => {
      matrixScenario = select.value;
      const el = $("scn-matrix");
      if (el) {
        el.innerHTML = matrixSectionHtml();
        wireMatrix(el);
      }
    };
  }
  const result = (report.scenarios || []).find((s) => s.scenario.name === matrixScenario);
  if (!result || !root) return;
  for (const b of root.querySelectorAll("[data-matrix-scope]")) {
    b.onclick = () => openMatrixCell(result, b.dataset.matrixScope, b.dataset.matrixFw);
  }
}

// ------------------------------------------------------------------ promote

function scopeOptionsHtml(selected) {
  return (
    `<option value="">(choose a scope)</option>` +
    (state.scopes || []).map((s) => `<option value="${esc(s.name)}" ${s.name === selected ? "selected" : ""}>${esc(s.name)}</option>`).join("")
  );
}

/// A small local mirror of `task-form.js`'s own `agentOptions` -- not
/// imported, so a Node test of this file does not also have to satisfy
/// `task-form.js`'s own import graph (schedule parsing, the create form's
/// DOM) for a two-option `<select>`.
function agentOptionsHtml(scopeName, selected) {
  const scope = (state.scopes || []).find((s) => s.name === scopeName);
  const out = [`<option value="">(scope default)</option>`];
  const named = new Set();
  if (scope) {
    for (const a of scope.agents) {
      named.add(a.name);
      out.push(`<option value="${esc(a.name)}" ${a.name === selected ? "selected" : ""}>${esc(a.name)}</option>`);
    }
  }
  const rest = (state.adapters || []).filter((a) => !named.has(a));
  if (rest.length) {
    out.push(
      `<optgroup label="any adapter">${rest.map((a) => `<option value="${esc(a)}" ${a === selected ? "selected" : ""}>${esc(a)}</option>`).join("")}</optgroup>`,
    );
  }
  return out.join("");
}

function taskHref(scope, taskId) {
  return refLinks(scope, [{ kind: "task", id: taskId }])[0].href;
}

function promoteResultHtml(result) {
  const created = result.created
    .map((c) => `<li><code class="id">${esc(c.control)}</code> — <a href="${esc(taskHref(result.scope, c.task.id))}">${esc(c.task.title)}</a></li>`)
    .join("");
  const skipped = result.skipped
    .map((s) => `<li><code class="id">${esc(s.control)}</code> — already <a href="${esc(taskHref(result.scope, s.existing_task))}">open</a></li>`)
    .join("");
  return `
    <h3 class="pol-sub-head">Created (${result.created.length})</h3>
    ${result.created.length ? `<ul>${created}</ul>` : `<div class="empty">Nothing to create -- every newly-open control here already has an open task.</div>`}
    <h3 class="pol-sub-head">Skipped (${result.skipped.length})</h3>
    ${result.skipped.length ? `<ul>${skipped}</ul>` : `<div class="empty">Nothing skipped.</div>`}`;
}

async function submitPromote(scenarioName) {
  const err = $("pm-err");
  if (err) err.textContent = "";
  const scope = $("pm-scope").value;
  if (!scope) {
    if (err) err.textContent = "Choose a scope.";
    return;
  }
  const agent = $("pm-agent").value || undefined;
  const button = $("pm-submit");
  if (button) button.disabled = true;
  try {
    const answer = await api("/api/scenarios/promote", { method: "POST", body: JSON.stringify(promoteBody(scenarioName, scope, agent)) });
    const resultEl = $("pm-result");
    if (resultEl) resultEl.innerHTML = promoteResultHtml(answer.result);
  } catch (e) {
    if (err) err.textContent = e.message;
  } finally {
    if (button) button.disabled = false;
  }
}

function openPromoteModal(scenarioName) {
  dropModal();
  const scope = state.scope || (state.scopes[0] && state.scopes[0].name) || "";
  scrim(`
    <header><div><h2>Promote ${esc(scenarioName)}</h2><span class="sub">creates one task per newly-open control in the chosen scope; a control that already has an open task is skipped, not refused</span></div>
      <button class="x" id="pm-close">&times;</button></header>
    <div class="body">
      <label for="pm-scope">Scope</label>
      <select id="pm-scope">${scopeOptionsHtml(scope)}</select>
      <label for="pm-agent">Agent <span class="sub" style="text-transform:none">optional</span></label>
      <select id="pm-agent">${agentOptionsHtml(scope, "")}</select>
      <div class="err" id="pm-err"></div>
      <div class="row-btns" style="margin-top:16px">
        <button class="btn primary" id="pm-submit">Promote</button>
        <button class="btn" id="pm-cancel">Cancel</button>
      </div>
      <div id="pm-result"></div>
    </div>`);
  $("pm-close").onclick = closeModal;
  $("pm-cancel").onclick = closeModal;
  $("pm-scope").onchange = () => {
    $("pm-agent").innerHTML = agentOptionsHtml($("pm-scope").value, "");
  };
  $("pm-submit").onclick = () => submitPromote(scenarioName);
}

// ----------------------------------------------------------------- workshop

function narrativeCardHtml(s) {
  return `<div class="scn-narrative-card">
    <div class="title">${esc(s.scenario.title)}</div>
    <code class="id">${esc(s.scenario.name)}</code>
    ${s.scenario.assumptions ? `<p class="sub">${esc(s.scenario.assumptions.trim())}</p>` : ""}
  </div>`;
}

function workshopHtml() {
  const board = narrativeBoard(report.scenarios);
  if (!board.quadrants.length) {
    return `<div class="empty">No narrative-kind scenarios yet. Add <code class="id">kind: [narrative]</code> to a
      scenario file with a <code class="id">narrative:</code> block naming its <code class="id">axes</code>,
      <code class="id">quadrant</code>, PESTLE <code class="id">drivers</code> and a <code class="id">premortem</code>
      list -- see README.md, "Scenarios".</div>`;
  }
  const axesLabel = board.axes ? board.axes.map(esc).join(" × ") : "axes not named";
  const quadrants = board.quadrants
    .map(
      (q) => `<div class="scn-quadrant">
      <h4>${esc(q.quadrant)}</h4>
      ${q.scenarios.map(narrativeCardHtml).join("")}
    </div>`,
    )
    .join("");
  const pestle = board.pestle
    .map(
      (p) => `<div class="scn-pestle-group"><div class="sub">${esc(p.scenario)}</div>
      <div class="scn-chips">${p.drivers.map((d) => `<span class="scn-chip">${esc(d)}</span>`).join("")}</div></div>`,
    )
    .join("");
  const premortem = board.premortem
    .map((p) => `<div class="scn-pestle-group"><div class="sub">${esc(p.scenario)}</div><ul>${p.points.map((x) => `<li>${esc(x)}</li>`).join("")}</ul></div>`)
    .join("");
  return `
    <div class="scn-axes-label">${esc(axesLabel)}</div>
    <div class="scn-quadrants">${quadrants}</div>
    <h3 class="pol-sub-head">PESTLE drivers</h3>
    ${pestle || `<div class="empty">None named.</div>`}
    <h3 class="pol-sub-head">Pre-mortem</h3>
    ${premortem || `<div class="empty">None named.</div>`}`;
}

// ------------------------------------------------------------------ findings

function findingsHtml() {
  const all = [...(report.findings || []).map((f) => ({ ...f, source: "scenario" })), ...(report.policy_findings || []).map((f) => ({ ...f, source: "policy" }))];
  if (!all.length) return `<div class="empty">No findings.</div>`;
  return `<ul class="scn-findings">${all
    .map((f) => `<li><code class="id">${esc(f.subject)}</code> — ${esc(f.detail)} <span class="sub">(${esc(f.kind)})</span></li>`)
    .join("")}</ul>`;
}

// --------------------------------------------------------------------- mode

function wireModeSeg() {
  const seg = $("scn-mode");
  if (!seg) return;
  for (const b of seg.querySelectorAll("[data-mode]")) {
    b.onclick = () => {
      mode = b.dataset.mode;
      for (const x of seg.querySelectorAll("[data-mode]")) x.classList.toggle("on", x === b);
      const boardWrap = $("scn-board-wrap");
      const workshopWrap = $("scn-workshop-wrap");
      if (boardWrap) boardWrap.hidden = mode !== "board";
      if (workshopWrap) workshopWrap.hidden = mode !== "workshop";
    };
  }
}

// -------------------------------------------------------------------- board

export function renderScenarios() {
  const note = $("scn-scope-note");
  if (note) {
    note.textContent = state.scope === null ? "No scope selected: this is the whole instance." : `${state.scope} and every scope below it.`;
  }
  const failed = $("scn-error");
  if (failed) {
    failed.textContent = reportError || "";
    failed.hidden = !reportError;
  }
  const countEl = $("scn-count");

  if (!report) {
    if (countEl) countEl.textContent = "";
    for (const id of ["scn-signposts", "scn-board", "scn-delta", "scn-drivers", "scn-matrix", "scn-workshop", "scn-findings"]) {
      const el = $(id);
      if (el) el.innerHTML = "";
    }
    return;
  }

  const scenarios = report.scenarios || [];
  if (countEl) countEl.textContent = `${scenarios.length} scenario${scenarios.length === 1 ? "" : "s"}`;

  const stripEl = $("scn-signposts");
  if (stripEl) stripEl.innerHTML = signpostStripHtml();

  const boardEl = $("scn-board");
  if (boardEl) {
    if (!scenarios.length) {
      boardEl.innerHTML = emptyStateHtml();
    } else {
      boardEl.innerHTML = baselineCardHtml() + scenarios.map(scenarioCardHtml).join("");
      wireBoardButtons(boardEl);
    }
  }

  const deltaEl = $("scn-delta");
  if (deltaEl) deltaEl.innerHTML = scenarios.length ? deltaTableHtml() : "";

  renderDriverPanel();

  const matrixEl = $("scn-matrix");
  if (matrixEl) {
    matrixEl.innerHTML = matrixSectionHtml();
    wireMatrix(matrixEl);
  }

  const workshopEl = $("scn-workshop");
  if (workshopEl) workshopEl.innerHTML = workshopHtml();

  const findingsEl = $("scn-findings");
  if (findingsEl) findingsEl.innerHTML = findingsHtml();
}

async function loadMetricDefs() {
  const ids = [...new Set((report && report.baseline.metrics || []).map((m) => m.id))];
  if (!ids.length) {
    metricDefs = {};
    return;
  }
  try {
    const answer = await api(`/api/metrics?ids=${ids.map(encodeURIComponent).join(",")}`);
    const defs = {};
    for (const d of answer.registry || []) defs[d.id] = d;
    metricDefs = defs;
  } catch (e) {
    // Best-effort: `metricTitle`'s own fallback covers a missing registry
    // entry, so a failed second fetch never blocks the tab on its own.
    metricDefs = {};
  }
}

export async function loadScenarios() {
  const mine = ++asked;
  ++whatifAsked;
  if (whatifTimer) clearTimeout(whatifTimer);
  report = null;
  whatifResult = null;
  whatifError = null;
  renderScenarios();
  const query = state.scope === null ? "" : `?scope=${encodeURIComponent(state.scope)}`;
  try {
    const answer = await api(`/api/scenarios${query}`);
    if (mine !== asked) return;
    report = answer.report;
    reportError = null;
    await loadMetricDefs();
    if (mine !== asked) return;
  } catch (e) {
    if (mine !== asked) return;
    report = null;
    reportError = e.message;
  }
  // A stale drill-down selection naming a scenario the new answer no longer
  // has would otherwise render nothing rather than falling back honestly.
  const names = new Set((report && report.scenarios || []).map((s) => s.scenario.name));
  if (driverScenario && !names.has(driverScenario)) driverScenario = null;
  if (!matrixScenario || !names.has(matrixScenario)) matrixScenario = report && report.scenarios.length ? report.scenarios[0].scenario.name : null;
  moved = {};
  whatifResult = null;
  whatifError = null;
  renderScenarios();
  if (report) await doWhatif();
}

/// `policy_changed`/`goals_changed`: a scenario's own policy delta and goal
/// probabilities are computed over the same live evidence and check-ins
/// those tabs change. Not `run_updated`/`task_created`/`task_updated`: a
/// scenario's forecast is a Monte Carlo draw over `/api/production` history
/// recomputed per scope per scenario, not a cheap re-render over what is
/// already on screen, so a run finishing leaves Refresh to catch up rather
/// than firing a reload on every one of them.
export function reloadScenarios() {
  void loadScenarios();
}

export function wireScenarios() {
  const button = $("scn-refresh");
  if (button) button.onclick = () => void loadScenarios();
  wireModeSeg();
}
