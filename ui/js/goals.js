//! The L6 Direction, Goals tab (GitHub issue `#99`, slice 3): vision,
//! mission and objectives as a live strategy map, a roadmap, an optional
//! orbit view and an accessible table -- never a policy engine, never a
//! process gate (design §8 holds here exactly as it does for the Policy
//! tab). `goals-model.js` holds every pure shaping function this file draws
//! from; this file only fetches, measures the DOM, draws, and wires clicks
//! -- the same split `policy.js` keeps, for the same reason: a Node test
//! needs no DOM to exercise the shaping logic.
//!
//! Rail semantics match Policy's: nothing selected fetches the whole
//! instance (`GET /api/goals`), a scope selected fetches
//! `GET /api/goals?scope=<name>` (that scope and its descendants) -- the
//! daemon narrows itself (`Engine::goals_report`).
//!
//! No poll: a goals read is cheap (two small YAML files, a handful of
//! already-cached metric computations) and changes only on a check-in,
//! which arrives as `goals_changed` (wired in `app.js`, the same way
//! `policy_changed` drives `reloadPolicy`).

import { $, api, esc, state } from "./core.js";
import { scrim, closeModal, dropModal } from "./modal.js";
import {
  KR_RING_RADIUS,
  alignsToEdges,
  bandBadgeClass,
  bandLabel,
  checkinBody,
  connectorPath,
  cycleOptionLabel,
  cycleProgress,
  danglingAlignsTo,
  defaultCycleId,
  findingLabel,
  findingsByKind,
  formatUnitValue,
  hasDirection,
  krHistoryValues,
  krScoredCount,
  krSourceLink,
  metricIdsForReport,
  metricUnavailableNote,
  normalizeViewMode,
  objectiveColorVar,
  objectiveLayers,
  orbitLayout,
  registryIndex,
  ringGeometry,
  ringDashArray,
  roadmapLanes,
  seriesIndex,
  sparklinePointsAttr,
  tableRows,
} from "./goals-model.js";

// ------------------------------------------------------------------ state

/// Answers can arrive out of order when the rail or the cycle switcher moves
/// quickly; only the newest request is allowed to draw -- the same guard
/// `policy.js`'s `loadPolicy` uses for the same reason.
let asked = 0;

/// The segmented control's current segment, remembered per browser --
/// ephemeral UI state, not routed into the URL (the same choice `policy.js`'s
/// control-detail modal and `datasets.js`'s "Add case" form make: this file's
/// header comment is the one place that says so).
const VIEW_MODE_KEY = "factory-goals-view";
function readViewMode() {
  try {
    return normalizeViewMode(localStorage.getItem(VIEW_MODE_KEY));
  } catch (e) {
    return "map"; // a private window, or storage disabled -- start on the landing view
  }
}
function writeViewMode(mode) {
  try {
    localStorage.setItem(VIEW_MODE_KEY, mode);
  } catch (e) {
    /* private window */
  }
}
let viewMode = readViewMode();

/// The key result detail modal currently open, `{kr}` (a `"objective/kr"`
/// string) or `null` -- also ephemeral, like `policy.js`'s `openControl`.
let openKr = null;
let krAsked = 0;

// ------------------------------------------------------------------- load

export async function loadGoals() {
  const mine = ++asked;
  const params = new URLSearchParams();
  if (state.scope !== null) params.set("scope", state.scope);
  if (state.goalsCycle) params.set("cycle", state.goalsCycle);
  const query = params.toString() ? `?${params.toString()}` : "";
  try {
    const answer = await api(`/api/goals${query}`);
    if (mine !== asked) return;
    state.goals = answer.report;
    state.goalsError = null;
  } catch (e) {
    if (mine !== asked) return;
    state.goals = null;
    state.goalsMetrics = null;
    state.goalsError = e.message;
    renderGoals();
    return;
  }

  // A second, small fetch for the ids the report actually names -- the
  // registry (units) and the three production metrics' own series (see the
  // module doc comment on honest sparklines). Its own failure never hides
  // the report already in hand; the map just draws without units or trend
  // lines for this round.
  try {
    const ids = metricIdsForReport(state.goals);
    const metricsAnswer = ids.length ? await api(`/api/metrics?ids=${ids.map(encodeURIComponent).join(",")}`) : null;
    if (mine !== asked) return;
    state.goalsMetrics = metricsAnswer;
  } catch (e) {
    if (mine !== asked) return;
    state.goalsMetrics = null;
  }
  renderGoals();
}

/// `goals_changed` (a check-in recorded): the report's own key result can
/// change, and the open detail modal, if any, is showing exactly the thing
/// that just changed.
export function reloadGoals() {
  loadGoals();
  if (openKr && $("kr-body")) refreshKrDetail(krAsked);
}

export function wireGoals() {
  const refresh = $("goals-refresh");
  if (refresh) refresh.onclick = () => loadGoals();
  const select = $("goal-cycle-select");
  if (select) {
    select.onchange = () => {
      state.goalsCycle = select.value || null;
      loadGoals();
    };
  }
  const seg = $("goal-view-seg");
  if (seg) {
    for (const b of seg.querySelectorAll("button[data-view]")) {
      b.onclick = () => setViewMode(b.dataset.view);
    }
  }
}

function setViewMode(mode) {
  const next = normalizeViewMode(mode);
  if (next === viewMode) return;
  viewMode = next;
  writeViewMode(viewMode);
  renderGoals();
}

// ------------------------------------------------------------------ render

export function renderGoals() {
  const note = $("goals-scope-note");
  if (note) {
    note.textContent = state.scope === null
      ? "No scope selected: this is the whole instance's strategy."
      : `${state.scope} and every scope below it.`;
  }
  const failed = $("goals-error");
  if (failed) {
    failed.textContent = state.goalsError || "";
    failed.hidden = !state.goalsError;
  }

  const report = state.goalsError ? null : state.goals;
  const empty = $("goals-empty");
  const body = $("goals-body");
  if (!report || !hasDirection(report)) {
    if (empty) empty.hidden = !report; // a fetch error already says why; do not also claim nothing is authored
    if (body) body.hidden = true;
    return;
  }
  if (empty) empty.hidden = true;
  if (body) body.hidden = false;

  renderHeader(report);
  renderSegControl();
  renderMap(report);
  renderRoadmap(report);
  renderOrbit(report);
  renderTable(report);
  renderFindings(report.findings);
}

// -------------------------------------------------------------- header

function renderHeader(report) {
  const d = report.direction;
  const visionEl = $("goal-vision");
  if (visionEl) visionEl.textContent = d.vision;
  const missionEl = $("goal-mission");
  if (missionEl) missionEl.textContent = d.mission;
  const valuesEl = $("goal-values");
  if (valuesEl) valuesEl.innerHTML = (d.values || []).map((v) => `<span class="goal-chip">${esc(v)}</span>`).join("");

  const select = $("goal-cycle-select");
  if (select) {
    const selected = state.goalsCycle || (report.report ? report.report.cycle_id : defaultCycleId(report.cycles));
    select.innerHTML = (report.cycles || [])
      .map((c) => `<option value="${esc(c.id)}"${c.id === selected ? " selected" : ""}>${esc(cycleOptionLabel(c))}</option>`)
      .join("");
  }

  const noteEl = $("goals-cycle-note");
  const cycleSummary = report.report && (report.cycles || []).find((c) => c.id === report.report.cycle_id);
  if (noteEl) {
    noteEl.textContent = cycleSummary ? `${cycleSummary.from} – ${cycleSummary.to}` : report.report ? "" : "no cycle to show";
  }

  const progressEl = $("goal-progress");
  if (progressEl) progressEl.innerHTML = report.report ? progressBarHtml(cycleProgress(report.report), krScoredCount(report.report)) : "";
}

/// `progress.scorePct` (the mean of every objective's own score -- see
/// `cycleProgress`'s header comment) is the one "score" this whole tab ever
/// shows, so the cycle switcher and this bar can never read two different
/// numbers for the same cycle again. `counted` ("n of m KRs scored") is a
/// plain count, not another score, and is labelled as one on purpose.
function progressBarHtml(progress, counted) {
  if (!progress) return "";
  const fill = progress.scorePct === null ? 0 : progress.scorePct;
  const scoreLabel = progress.scorePct === null ? "not scored yet" : `${progress.scorePct}% score`;
  const countedLabel = `${counted.scored} of ${counted.total} KRs scored`;
  return `<div class="goal-bar-label"><span>${esc(scoreLabel)}</span><span>${progress.elapsedPct}% of the cycle elapsed</span></div>
    <div class="goal-bar" role="img" aria-label="${esc(scoreLabel)}, ${progress.elapsedPct}% of the cycle elapsed">
      <div class="goal-bar-fill" style="width:${fill}%"></div>
      <div class="goal-bar-marker" style="left:${progress.elapsedPct}%" title="expected by now"></div>
    </div>
    <div class="sub goal-bar-counted">${esc(countedLabel)}</div>`;
}

// --------------------------------------------------------- segmented control

function renderSegControl() {
  const seg = $("goal-view-seg");
  if (seg) {
    for (const b of seg.querySelectorAll("button[data-view]")) {
      const on = b.dataset.view === viewMode;
      b.classList.toggle("on", on);
      b.setAttribute("aria-selected", on ? "true" : "false");
    }
  }
  for (const name of ["map", "roadmap", "orbit", "table"]) {
    const section = $(`goal-view-${name}`);
    if (section) section.hidden = name !== viewMode;
  }
}

// ------------------------------------------------------------------- map

const BAND_COLOR = { green: "var(--run)", yellow: "var(--wait)", red: "var(--fault)" };
function bandColorVar(band) {
  return BAND_COLOR[band] || "var(--idle)";
}

// A key result's own ring: ~64px (`RING_SIZE`), the size the design pass
// asked for -- bigger than a glance-only indicator, small enough that a
// `minmax(260px,1fr)` card still reads as compact. `OBJ_RING_SIZE` is the
// same picture at a third the size, for an objective's own aggregate score
// beside its title.
const RING_SIZE = 64;
const RING_R = KR_RING_RADIUS;
const OBJ_RING_SIZE = 40;
const OBJ_RING_R = 16;

/// The one ring renderer both a key result's and an objective's own ring
/// call into -- three states, never two: a scored ring (committed solid /
/// aspirational dashed, coloured by band, the score centred as text) and an
/// *unscored* ring, which is not "a ring at score 0" (that already exists
/// and looks different: a fully closed, coloured circle) but a distinct
/// picture entirely -- a faint dashed outline with no fill arc at all and
/// "—" centred, so "no evidence yet" can never be mistaken for "scored
/// zero" at a glance. The faint track circle is drawn underneath every
/// state, scored or not.
function ringSvgCore({ size, radius, score, dashedTrack, color, outerClass, textClass }) {
  const c = size / 2;
  const unscored = score === null || score === undefined;
  const centerText = unscored ? "—" : `${Math.round(score * 100)}%`;
  const textClasses = ["goal-ring-text", textClass, unscored ? "goal-ring-text-unscored" : ""].filter(Boolean).join(" ");
  const trackDash = dashedTrack ? ringDashArray("aspirational", 2 * Math.PI * radius) : null;
  const track = `<circle class="goal-ring-track${dashedTrack ? " goal-ring-track-aspirational" : ""}" cx="${c}" cy="${c}" r="${radius}"${trackDash ? ` stroke-dasharray="${trackDash}"` : ""}></circle>`;
  const text = `<text class="${textClasses}" x="${c}" y="${c}" dy="0.32em" text-anchor="middle">${centerText}</text>`;
  let body;
  if (unscored) {
    body = `<circle class="goal-ring-unscored" cx="${c}" cy="${c}" r="${radius}"></circle>`;
  } else {
    const { circumference, offset } = ringGeometry(score, radius);
    body = `<circle class="goal-ring-fill" cx="${c}" cy="${c}" r="${radius}" stroke="${color}"
      stroke-dasharray="${circumference}" stroke-dashoffset="${circumference}"
      data-target-offset="${offset}" transform="rotate(-90 ${c} ${c})"></circle>`;
  }
  return `<svg class="goal-ring-svg${outerClass ? ` ${outerClass}` : ""}" viewBox="0 0 ${size} ${size}" width="${size}" height="${size}" aria-hidden="true">
    ${track}${body}${text}
  </svg>`;
}

function ringSvg(kr) {
  return ringSvgCore({ size: RING_SIZE, radius: RING_R, score: kr.score, dashedTrack: kr.kind === "aspirational", color: bandColorVar(kr.band) });
}

/// An objective's own score has no band (committed/aspirational is a key
/// result's own distinction, not something a mean of several has), so this
/// always draws solid, in the signal colour rather than a band colour --
/// visually reads as "an aggregate", not "another key result".
function objectiveRingSvg(score) {
  return ringSvgCore({
    size: OBJ_RING_SIZE,
    radius: OBJ_RING_R,
    score,
    dashedTrack: false,
    color: "var(--signal)",
    outerClass: "goal-obj-ring-svg",
    textClass: "goal-obj-ring-text",
  });
}

function sparklineSvg(values, color, extraClass) {
  const pts = sparklinePointsAttr(values);
  if (!pts) return `<span class="sub goal-no-history">no history yet</span>`;
  return `<svg class="goal-spark${extraClass ? ` ${extraClass}` : ""}" viewBox="0 0 100 26" preserveAspectRatio="none">
    <polyline points="${pts}" fill="none" stroke="${color}" stroke-width="2"></polyline>
  </svg>`;
}

function sparkSvg(kr) {
  const values = krHistoryValues(kr, state.goals.checkins, seriesIndex(state.goalsMetrics || {}));
  return sparklineSvg(values, bandColorVar(kr.band));
}

/// A metric's own daily series (only ever present for the three production
/// metrics -- see `goals-model.js`'s header comment), read straight off
/// `/api/metrics`' `series`, never invented for a metric with none.
function metricSparkSvg(metricId, color, extraClass) {
  const series = seriesIndex(state.goalsMetrics || {})[metricId];
  const values = series ? series.points.map(([, v]) => v) : null;
  return sparklineSvg(values, color, extraClass);
}

/// A compact card: the ring on the left, everything else in a column on the
/// right -- one row for the band badge and the kind (neither stretched to
/// the card's own width; both are wrapped in their own flex row rather than
/// left as bare flex children of a column, which is what was stretching the
/// badge full-width before), then value → target, the source (a real link
/// when `krSourceLink` finds one to route to, so a click does not also open
/// this card's own detail modal -- see `wireMapClicks`), then the sparkline.
function krCardHtml(kr) {
  const registry = registryIndex(state.goalsMetrics || {});
  const unit = kr.metric && registry[kr.metric] ? registry[kr.metric].unit : null;
  const valueLabel = formatUnitValue(kr.value, unit);
  const targetLabel = formatUnitValue(kr.target, unit);
  const link = krSourceLink(kr, state.scope);
  const sourceHtml = kr.manual
    ? `<span class="goal-kr-source sub">manual</span>`
    : link
      ? `<a class="goal-kr-source" href="${esc(link.href)}" data-stop-card>${esc(kr.metric)} → ${esc(link.label)}</a>`
      : `<span class="goal-kr-source sub">${esc(kr.metric || "")}</span>`;
  return `<div class="goal-kr" data-kr="${esc(kr.kr)}" role="button" tabindex="0">
    <div class="goal-kr-ring">${ringSvg(kr)}</div>
    <div class="goal-kr-body">
      <div class="goal-kr-title">${esc(kr.title)}</div>
      <div class="goal-kr-badges">
        <span class="badge ${bandBadgeClass(kr.band)}">${esc(bandLabel(kr.band))}</span>
        <span class="goal-kr-kind">${esc(kr.kind)}${kr.onPace === false ? " · behind pace" : ""}</span>
        ${kr.confidence !== null && kr.confidence !== undefined ? `<span class="goal-kr-kind">confidence ${kr.confidence}/10</span>` : ""}
      </div>
      <div class="sub">${valueLabel} → ${targetLabel}</div>
      ${sourceHtml}
      ${sparkSvg(kr)}
    </div>
  </div>`;
}

function objectiveCardHtml(o, index, objectivesOnScreen) {
  const dangling = danglingAlignsTo(objectivesOnScreen).find((d) => d.objective === o.objective);
  return `<article class="goal-obj" data-objective="${esc(o.objective)}" style="--obj-color:${objectiveColorVar(index)}">
    <header class="goal-obj-head">
      <div class="goal-obj-ring">${objectiveRingSvg(o.score)}</div>
      <div class="goal-obj-head-text">
        <h4>${esc(o.title)}</h4>
        <div class="goal-obj-head-meta">
          ${o.scope ? `<span class="goal-chip goal-obj-scope">${esc(o.scope)}</span>` : ""}
          ${dangling ? `<span class="sub goal-dangling">aligns to ${esc(dangling.alignsTo)} — outside this scope</span>` : ""}
        </div>
      </div>
    </header>
    <div class="goal-krs">${(o.key_results || []).map(krCardHtml).join("")}</div>
  </article>`;
}

/// The North Star as a hero: a large value (and its own sparkline, when the
/// metric behind it is one of the three the daemon computes a series for --
/// `first_pass_yield` always is), the "why" prose beside it, no gauge
/// against a target since `NorthStar` (`direction.yaml`) carries no
/// `target` field to gauge against -- only ever "just the value", which is
/// this function's only path, not a fallback branch of a missing one.
/// Inputs read their titles from the metrics registry (falling back to the
/// bare id before the registry has answered), each with its own sparkline
/// where one exists.
function northStarRowHtml(report) {
  if (!report.north_star) return "";
  const registry = registryIndex(state.goalsMetrics || {});
  const nsUnit = registry[report.north_star.metric] ? registry[report.north_star.metric].unit : null;
  const nsNote = metricUnavailableNote(report.north_star.value);
  const nsValue = nsNote ? nsNote : formatUnitValue(report.north_star.value.value, nsUnit);
  const inputs = (report.inputs || [])
    .map((i) => {
      const def = registry[i.metric];
      const title = def ? def.title : i.metric;
      const unit = def ? def.unit : null;
      const note = metricUnavailableNote(i.value);
      const value = note ? note : formatUnitValue(i.value.value, unit);
      return `<div class="goal-input-box">
        <div class="goal-input-title">${esc(title)}</div>
        <div class="goal-input-id">${esc(i.metric)}</div>
        <div class="goal-input-value">${esc(value)}</div>
        ${metricSparkSvg(i.metric, "var(--signal-ink)")}
      </div>`;
    })
    .join("");
  return `<div class="goal-northstar-row">
    <div class="goal-ns-hero">
      <div class="sub">North star</div>
      <div class="goal-ns-value">${esc(nsValue)}</div>
      <div class="goal-ns-id">${esc(report.north_star.metric)}</div>
      ${metricSparkSvg(report.north_star.metric, "var(--signal)", "goal-ns-spark")}
      <p class="sub goal-ns-why">${esc(report.north_star.why)}</p>
    </div>
    ${inputs ? `<div class="goal-ns-arrow" aria-hidden="true">→</div><div class="goal-inputs">${inputs}</div>` : ""}
  </div>`;
}

function legendHtml() {
  return `<span class="goal-legend-item"><span class="goal-legend-ring goal-legend-solid"></span>committed (solid ring, green only once met)</span>
    <span class="goal-legend-item"><span class="goal-legend-ring goal-legend-dashed"></span>aspirational (dashed track, green once clearly winning)</span>
    <span class="goal-legend-item"><span class="goal-legend-ring goal-legend-unscored"></span>unscored (no evidence yet)</span>
    <span class="goal-legend-item"><span class="badge s-green">green</span><span class="badge s-yellow">yellow</span><span class="badge s-red">red</span><span class="badge s-unscored">unscored</span></span>`;
}

/// A thin vertical line with a dot per section -- North Star, Inputs (when
/// there are any) and one per cascade layer of objectives -- so the map
/// reads top to bottom as one line of descent rather than a stack of
/// unrelated boxes. Evenly spaced (`justify-content: space-between` in CSS)
/// rather than measured against each section's real position: a spine that
/// approximates the cascade is worth having, and re-measuring it on every
/// resize alongside the `aligns_to` connectors is not.
function spineHtml(sectionCount) {
  const dots = Array.from({ length: Math.max(sectionCount, 1) }, () => `<span class="goal-spine-dot"></span>`).join("");
  return `<div class="goal-spine" aria-hidden="true"><span class="goal-spine-line"></span>${dots}</div>`;
}

function renderMap(report) {
  const mapEl = $("goal-map");
  if (!mapEl) return;
  const objectives = (report.report && report.report.objectives) || [];
  const layers = objectiveLayers(objectives);
  const layersHtml = layers
    .map((layer) => `<div class="goal-layer">${layer.map((o) => objectiveCardHtml(o, objectives.indexOf(o), objectives)).join("")}</div>`)
    .join("");
  const sectionCount = (report.north_star ? 1 : 0) + ((report.inputs || []).length ? 1 : 0) + layers.length;
  mapEl.innerHTML = `<div class="goal-map-flow">
    ${spineHtml(sectionCount)}
    <div class="goal-map-content">
      ${northStarRowHtml(report)}
      <div class="goal-layers-wrap" id="goal-layers-wrap">
        <svg class="goal-connectors" id="goal-connectors"></svg>
        ${layersHtml}
      </div>
    </div>
  </div>`;
  const legendEl = $("goal-legend");
  if (legendEl) legendEl.innerHTML = legendHtml();

  wireMapClicks(mapEl);
  animateRings(mapEl);
  drawMapConnectors(objectives);
}

/// A `.goal-kr` card is a `div[role=button]`, not a real `<button>`, because
/// it carries its own source link (`krCardHtml`) and interactive content
/// cannot nest inside a `<button>` -- the same reason the orbit view's own
/// key-result circles (`renderOrbit`) are focusable SVG shapes rather than
/// buttons. The embedded link stops the click from bubbling to the card's
/// own handler, so following it does not also pop the detail modal open
/// behind the navigation.
function wireMapClicks(root) {
  for (const el of root.querySelectorAll("[data-kr]")) {
    el.onclick = () => openKrDetail(el.dataset.kr);
    el.onkeydown = (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        openKrDetail(el.dataset.kr);
      }
    };
  }
  for (const a of root.querySelectorAll("[data-stop-card]")) {
    a.onclick = (e) => e.stopPropagation();
  }
}

/// Rings are drawn fully empty (`stroke-dashoffset` at the full
/// circumference) and animated to their real score after two animation
/// frames -- a browser paints the empty state on the first frame, so the
/// second is the transition's own starting point, the standard trick for
/// animating a value set at insertion time. `prefers-reduced-motion` is
/// handled entirely in CSS (the transition itself is removed there), so this
/// never has to ask `matchMedia` -- the same final value is set either way.
function animateRings(root) {
  const circles = root.querySelectorAll ? [...root.querySelectorAll(".goal-ring-fill")] : [];
  if (!circles.length) return;
  const apply = () => {
    for (const c of circles) {
      const target = c.dataset ? c.dataset.targetOffset : undefined;
      if (target !== undefined && c.style) c.style.strokeDashoffset = target;
    }
  };
  if (typeof requestAnimationFrame === "function") {
    requestAnimationFrame(() => requestAnimationFrame(apply));
  } else {
    apply();
  }
}

/// The `aligns_to` cascade's own connectors: a CSS grid places the objective
/// cards, so the lines between them are drawn afterwards, from measured
/// `getBoundingClientRect()`s, into an absolutely-positioned `<svg>` overlay
/// sized to the same wrapper -- the only way to connect two elements a grid
/// laid out, and why `connectorPath` (`goals-model.js`) takes plain rects
/// rather than elements. No-ops quietly wherever `getBoundingClientRect`
/// is not a real measurement (a Node test's bare DOM shim).
function drawMapConnectors(objectives) {
  const wrap = $("goal-layers-wrap");
  const svg = $("goal-connectors");
  if (!wrap || !svg || typeof wrap.getBoundingClientRect !== "function") return;
  const containerRect = wrap.getBoundingClientRect();
  svg.setAttribute("width", String(containerRect.width));
  svg.setAttribute("height", String(containerRect.height));
  svg.setAttribute("viewBox", `0 0 ${containerRect.width} ${containerRect.height}`);

  const rectById = new Map();
  for (const el of wrap.querySelectorAll("[data-objective]")) {
    rectById.set(el.dataset.objective, el.getBoundingClientRect());
  }
  const paths = alignsToEdges(objectives)
    .map((edge) => {
      const parentRect = rectById.get(edge.to);
      const childRect = rectById.get(edge.from);
      if (!parentRect || !childRect) return "";
      return `<path class="goal-connector" d="${connectorPath(parentRect, childRect, containerRect)}"></path>`;
    })
    .join("");
  svg.innerHTML = paths;
}

/// Redraws the map's connectors on resize while the map is the view showing
/// -- a CSS grid can reflow the cards' positions at any width, and the
/// overlay has to track them. One listener for the page's whole lifetime,
/// same as `terminal.js`'s own resize handling, rather than adding and
/// removing one on every view switch.
if (typeof window !== "undefined" && typeof window.addEventListener === "function") {
  window.addEventListener("resize", () => {
    if (state.tab === "goals" && viewMode === "map" && state.goals && state.goals.report) {
      drawMapConnectors(state.goals.report.objectives);
    }
  });
}

// --------------------------------------------------------------- roadmap

function roadmapCardHtml(item) {
  const colorVar = item.objectives.length && item.objectives[0].colorIndex !== null
    ? objectiveColorVar(item.objectives[0].colorIndex)
    : "var(--faint)";
  const progressLabel = item.progress === null ? "" : `<span class="badge">${Math.round(item.progress * 100)}%</span>`;
  return `<article class="goal-roadmap-card" style="--obj-color:${colorVar}">
    <div class="goal-roadmap-card-head"><h4>${esc(item.title)}</h4>${progressLabel}</div>
    ${item.orphan
      ? `<div class="sub goal-orphan">no objective linked — a finding below</div>`
      : `<div class="sub">${item.objectives.map((o) => esc(o.title)).join(", ")}</div>`}
    ${item.why ? `<p class="sub">${esc(item.why)}</p>` : ""}
    ${item.scope ? `<div class="sub">scope: ${esc(item.scope)}</div>` : ""}
  </article>`;
}

function renderRoadmap(report) {
  const el = $("goal-roadmap");
  if (!el) return;
  const lanes = roadmapLanes(report.roadmap, (report.report && report.report.objectives) || []);
  el.innerHTML = ["now", "next", "later"]
    .map(
      (lane) => `<div class="goal-lane" data-lane="${lane}">
        <div class="goal-lane-head">${lane}<span class="goal-lane-count">${lanes[lane].length}</span></div>
        <div class="goal-lane-body">${lanes[lane].length ? lanes[lane].map(roadmapCardHtml).join("") : `<div class="empty">Nothing here.</div>`}</div>
      </div>`,
    )
    .join("");
}

// ----------------------------------------------------------------- orbit

function renderOrbit(report) {
  const el = $("goal-orbit");
  if (!el) return;
  const objectives = (report.report && report.report.objectives) || [];
  const layout = orbitLayout(objectives, { width: 640, height: 640 });
  if (layout.tooMany) {
    el.innerHTML = `<div class="empty">The orbit view is a showpiece, readable to about 20 key results -- this cycle has
      ${layout.totalKrs}. Use the Map or Table view instead.</div>`;
    return;
  }
  if (!objectives.length) {
    el.innerHTML = `<div class="empty">Nothing to draw yet.</div>`;
    return;
  }
  const edges = layout.edges
    .map((e) => {
      const from = layout.nodes.find((n) => n.id === e.from);
      const to = layout.nodes.find((n) => n.id === e.to);
      if (!from || !to) return "";
      return `<line class="goal-orbit-edge" x1="${from.x}" y1="${from.y}" x2="${to.x}" y2="${to.y}"></line>`;
    })
    .join("");
  const nodes = layout.nodes
    .map((n) => {
      if (n.kind === "objective") {
        // The objective's own score as an arc around its node.
        const r = 18;
        const circ = 2 * Math.PI * r;
        const score = typeof n.score === "number" ? Math.max(0, Math.min(1, n.score)) : 0;
        return `<g class="goal-orbit-node" transform="translate(${n.x},${n.y})">
          <circle r="${r}" class="goal-orbit-obj"></circle>
          <circle r="${r}" class="goal-orbit-obj-arc" stroke-dasharray="${(circ * score).toFixed(1)} ${circ.toFixed(1)}"
            transform="rotate(-90)"></circle>
          <text class="goal-orbit-score" dy="0.32em">${Math.round(score * 100)}%</text>
          <text class="goal-orbit-label" y="32">${esc(truncate(n.title, 26))}</text>
          <title>${esc(n.title)}</title>
        </g>`;
      }
      // KR labels sit outside the ring, anchored away from the centre.
      const lx = n.x + 14 * Math.cos(n.angle);
      const ly = n.y + 14 * Math.sin(n.angle);
      const anchor = Math.cos(n.angle) > 0.2 ? "start" : Math.cos(n.angle) < -0.2 ? "end" : "middle";
      return `<g class="goal-orbit-kr-g">
        <circle class="goal-orbit-kr" data-kr="${esc(n.id)}" tabindex="0" role="button" aria-label="${esc(n.title)}"
          cx="${n.x}" cy="${n.y}" r="8" fill="${bandColorVar(n.band)}"
          stroke="${n.krKind === "aspirational" ? bandColorVar(n.band) : "none"}"
          stroke-dasharray="${n.krKind === "aspirational" ? "2 2" : "none"}"><title>${esc(n.title)}</title></circle>
        <text class="goal-orbit-kr-label" x="${lx.toFixed(1)}" y="${ly.toFixed(1)}" dy="0.32em" text-anchor="${anchor}">${esc(truncate(n.title, 20))}</text>
      </g>`;
    })
    .join("");
  el.innerHTML = `<svg class="goal-orbit-svg" viewBox="0 0 640 640" role="img" aria-label="Radial strategy map">
    <circle class="goal-orbit-guide" cx="${layout.cx}" cy="${layout.cy}" r="${(640 * 0.28).toFixed(1)}"></circle>
    <circle class="goal-orbit-guide" cx="${layout.cx}" cy="${layout.cy}" r="${(640 * 0.46).toFixed(1)}"></circle>
    ${layout.nodes.filter((n) => n.kind === "objective").map((n) =>
      `<line class="goal-orbit-spoke" x1="${layout.cx}" y1="${layout.cy}" x2="${n.x}" y2="${n.y}"></line>`).join("")}
    <circle class="goal-orbit-halo" cx="${layout.cx}" cy="${layout.cy}" r="30"></circle>
    <circle class="goal-orbit-centre" cx="${layout.cx}" cy="${layout.cy}" r="14"></circle>
    <text class="goal-orbit-centre-label" x="${layout.cx}" y="${layout.cy + 46}" text-anchor="middle">vision</text>
    ${edges}
    ${nodes}
  </svg>`;
  for (const c of el.querySelectorAll("[data-kr]")) {
    c.onclick = () => openKrDetail(c.dataset.kr);
    c.onkeydown = (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        openKrDetail(c.dataset.kr);
      }
    };
  }
}

function truncate(s, n) {
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
}

// ----------------------------------------------------------------- table

function tableRowHtml(row) {
  const registry = registryIndex(state.goalsMetrics || {});
  const unit = row.metric && registry[row.metric] ? registry[row.metric].unit : null;
  return `<tr class="row" data-kr="${esc(row.kr)}">
    <td>${esc(row.objectiveTitle)}</td>
    <td><div class="title">${esc(row.title)}</div><code class="id">${esc(row.kr)}</code></td>
    <td>${esc(row.kind)}${row.manual ? " · manual" : ""}</td>
    <td>${esc(formatUnitValue(row.value, unit))}</td>
    <td>${esc(formatUnitValue(row.target, unit))}</td>
    <td>${row.score === null || row.score === undefined ? "—" : `${Math.round(row.score * 100)}%`}</td>
    <td><span class="badge ${bandBadgeClass(row.band)}">${esc(bandLabel(row.band))}</span></td>
    <td class="sub">${esc(row.source)}</td>
  </tr>`;
}

function renderTable(report) {
  const body = $("goal-table");
  if (!body) return;
  const rows = tableRows(report);
  body.innerHTML = rows.map(tableRowHtml).join("");
  for (const tr of body.querySelectorAll("tr[data-kr]")) {
    tr.onclick = () => openKrDetail(tr.dataset.kr);
  }
}

// -------------------------------------------------------------- findings

function findingsHtml(findings) {
  const groups = findingsByKind(findings);
  return [...groups.entries()]
    .map(
      ([kind, rows]) => `<div class="pol-finding-group">
        <h4>${esc(findingLabel(kind))}</h4>
        <ul>${rows.map((f) => `<li><code>${esc(f.subject)}</code> — ${esc(f.detail)}</li>`).join("")}</ul>
      </div>`,
    )
    .join("");
}

function renderFindings(findings) {
  const el = $("goal-findings");
  if (el) el.innerHTML = findingsHtml(findings);
  const empty = $("goal-no-findings");
  if (empty) empty.hidden = (findings || []).length !== 0;
}

// ----------------------------------------------------------- detail modal

function findKr(krRef) {
  const objectives = (state.goals && state.goals.report && state.goals.report.objectives) || [];
  for (const o of objectives) {
    const kr = (o.key_results || []).find((k) => k.kr === krRef);
    if (kr) return { objective: o, kr };
  }
  return null;
}

function krDetailBodyHtml(objective, kr) {
  const registry = registryIndex(state.goalsMetrics || {});
  const unit = kr.metric && registry[kr.metric] ? registry[kr.metric].unit : null;
  const link = krSourceLink(kr, state.scope);
  const history = (state.goals.checkins && state.goals.checkins[kr.kr]) || [];
  return `
    <p class="sub">${esc(objective.title)}</p>
    <div class="goal-kr-detail-status">
      <span class="badge ${bandBadgeClass(kr.band)}">${esc(bandLabel(kr.band))}</span>
      <span class="sub">${esc(kr.kind)}</span>
      <span class="sub">value ${esc(formatUnitValue(kr.value, unit))} / target ${esc(formatUnitValue(kr.target, unit))}</span>
    </div>
    <p class="sub">${esc(kr.source)}</p>
    ${(kr.reasons || []).length ? `<ul class="pol-reasons">${kr.reasons.map((r) => `<li>${esc(r)}</li>`).join("")}</ul>` : ""}
    ${link ? `<p><a href="${esc(link.href)}">${esc(link.label)} →</a></p>` : kr.manual ? "" : `<p class="sub">no linked evidence view for this metric yet</p>`}
    ${sparkSvg(kr)}

    ${kr.manual
      ? `<h3 class="pol-sub-head">Check-in history</h3>
         ${history.length
           ? `<table class="pol-attestations"><thead><tr><th>Value</th><th>Confidence</th><th>By</th><th>When</th><th>Note</th></tr></thead>
              <tbody>${[...history].reverse().map(checkinRowHtml).join("")}</tbody></table>`
           : `<div class="empty">No check-ins yet.</div>`}
         <h3 class="pol-sub-head">Check in</h3>
         ${checkinFormHtml()}`
      : ""}`;
}

function checkinRowHtml(c) {
  return `<tr>
    <td>${esc(c.value)}</td>
    <td>${esc(c.confidence)}/10</td>
    <td>${esc(c.by)}</td>
    <td class="sub">${esc(new Date(c.at).toLocaleString())}</td>
    <td class="sub">${esc(c.note || "")}</td>
  </tr>`;
}

function checkinFormHtml() {
  return `
    <label for="kr-value">Value</label>
    <input id="kr-value" type="number" step="any">
    <label for="kr-confidence">Confidence <span class="sub" style="text-transform:none">0 (no evidence) to 10 (certain)</span></label>
    <input id="kr-confidence" type="range" min="0" max="10" step="1" value="5">
    <div class="sub" id="kr-confidence-value">5 / 10</div>
    <label for="kr-note">Note <span class="sub" style="text-transform:none">optional</span></label>
    <input id="kr-note">
    <div class="err" id="kr-err"></div>
    <div class="row-btns" style="margin-top:10px">
      <button class="btn primary" id="kr-checkin">Check in</button>
    </div>`;
}

function wireKrDetail(kr) {
  const confidence = $("kr-confidence");
  const confidenceValue = $("kr-confidence-value");
  if (confidence && confidenceValue) {
    confidence.oninput = () => { confidenceValue.textContent = `${confidence.value} / 10`; };
  }
  const button = $("kr-checkin");
  if (button) button.onclick = () => submitCheckin(kr.kr);
}

async function refreshKrDetail(mine) {
  if (!openKr) return;
  const body = $("kr-body");
  if (!body) return; // the modal was closed while a fetch was in flight
  const found = findKr(openKr.kr);
  if (mine !== krAsked || !openKr || !$("kr-body")) return;
  if (!found) {
    body.innerHTML = `<div class="empty">This key result is no longer in the asked cycle.</div>`;
    return;
  }
  const title = $("kr-title");
  if (title) title.textContent = found.kr.title;
  body.innerHTML = krDetailBodyHtml(found.objective, found.kr);
  animateRings(body);
  wireKrDetail(found.kr);
}

function openKrDetail(krRef) {
  dropModal();
  openKr = { kr: krRef };
  const mine = ++krAsked;
  scrim(`<header><div><h2 id="kr-title">${esc(krRef)}</h2><code class="id">${esc(krRef)}</code></div>
    <button class="x" id="kr-close">&times;</button></header>
    <div class="body" id="kr-body"><div class="empty">loading…</div></div>`);
  $("kr-close").onclick = () => { openKr = null; closeModal(); };
  refreshKrDetail(mine);
}

async function submitCheckin(krRef) {
  const err = $("kr-err");
  if (err) err.textContent = "";
  const button = $("kr-checkin");
  try {
    const body = checkinBody(krRef, {
      value: $("kr-value").value,
      confidence: $("kr-confidence").value,
      note: $("kr-note").value,
    });
    if (button) button.disabled = true;
    await api("/api/goals/checkins", { method: "POST", body: JSON.stringify(body) });
    // `goals_changed` will also arrive over the socket and reload the whole
    // report; this refetches right away so the person who just typed it
    // does not wait on that round trip to see their own check-in land.
    await loadGoals();
    await refreshKrDetail(krAsked);
  } catch (e) {
    if (err) err.textContent = e.message;
    const b = $("kr-checkin");
    if (b) b.disabled = false;
  }
}
