//! The L6 Direction, Quality attributes tab (GitHub issue `#107`, slice 3):
//! which qualities matter for each scope, how much, what they trade off
//! against, and whether they are met -- measured, never claimed. A picture,
//! not an enforcer (design §8): nothing here starts, stops or gates any
//! work, and the one write is an explicit "Create task".
//! `quality-model.js` holds every pure shaping function this file draws
//! from; this file only fetches, draws and wires clicks -- the same split
//! `goals.js`/`policy.js` keep, for the same reason: a Node test needs no DOM
//! to exercise the shaping logic.
//!
//! Five views over one answer, chosen with a segmented control remembered
//! per browser (like Goals' own): the company heatmap (landing), one
//! scope's utility tree, its trade-off matrix, its importance × difficulty
//! grid, and a table that says everything the pictures do in rows. The
//! three per-scope views share one scope picker; a click on a heatmap cell
//! sets it and opens the tree. Neither the view nor the picked scope is
//! routed into the URL -- ephemeral UI state, the same choice `goals.js`
//! makes for its own segmented control.
//!
//! Rail semantics match Goals' and Policy's: nothing selected fetches the
//! whole instance (`GET /api/quality`), a scope selected fetches
//! `GET /api/quality?scope=<name>` (that scope and its descendants) -- the
//! daemon narrows itself (`Engine::quality_report`).
//!
//! No poll. What reloads it is wired in `app.js`: `quality_changed` (the
//! profiles or a chain changed, noticed on some read), a run settling (a
//! fitness-function verdict or a production metric moving), and a task
//! carrying a `quality=` label (a remediation task opened or closed).
//!
//! No radar chart, no aggregate score -- the issue's guardrails, kept by
//! having nothing here that could draw one: bars, cells and small grids.

import { $, api, esc, state } from "./core.js";
import { refLinks } from "./policy-model.js";
import {
  LEVELS,
  attributeLabel,
  bulletGeometry,
  canRemediate,
  cellLabel,
  decisionLink,
  findingLabel,
  findingsByKind,
  focusScope,
  formatNumber,
  hasProfiles,
  heatmapRows,
  idTag,
  importanceDifficultyGrid,
  levelWord,
  measureKind,
  normalizeViewMode,
  openTaskFor,
  remediateBody,
  scenarioSentence,
  seriesPoints,
  seriesValues,
  sparkGeometry,
  statusLabel,
  tableRows,
  taskHref,
  tradeoffMatrix,
  utilityTree,
  valueDomain,
} from "./quality-model.js";

// ------------------------------------------------------------------ state

/// Answers can arrive out of order when the rail moves quickly; only the
/// newest request is allowed to draw -- the same guard `goals.js` uses.
let asked = 0;

/// The last answer's `.report`, and the error that replaced it. Module
/// state rather than `core.js`'s `state`: nothing outside this tab reads it.
let report = null;
let failure = null;

const VIEW_MODE_KEY = "factory-quality-view";
function readViewMode() {
  try {
    return normalizeViewMode(localStorage.getItem(VIEW_MODE_KEY));
  } catch (e) {
    return "heatmap"; // a private window, or storage disabled
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

/// The scope the per-scope views show, as last picked -- revalidated
/// against every fresh answer by `focusScope`, so a scope the rail narrowed
/// away never lingers on screen.
let picked = null;
/// The characteristic a heatmap click asked the tree to bring into view,
/// consumed by the next tree render.
let spotlight = null;
/// The trade-off pair whose detail is open, `"a|b"`, or `null`.
let openPair = null;

/// What a person is in the middle of, kept here rather than in the DOM so a
/// reload -- which the socket can fire at any moment, and which redraws the
/// whole tree -- never throws it away. `remediation` is keyed by
/// `remediationKey` and holds `{phase: "confirming" | "pending" | "error" |
/// "done", message?, task?, created?}`; a scenario absent from it shows its
/// plain "Create task". `collapsed` holds the tree nodes a person closed,
/// by `nodeKey`, so a redraw reopens nothing.
const remediation = new Map();
const collapsed = new Set();

function remediationKey(scope, attribute, scenario) {
  return JSON.stringify([scope, attribute, scenario]);
}
function nodeKey(scope, kind, id) {
  return JSON.stringify([scope, kind, id]);
}

// ------------------------------------------------------------------- load

export async function loadQuality() {
  const mine = ++asked;
  const query = state.scope === null ? "" : `?scope=${encodeURIComponent(state.scope)}`;
  try {
    const answer = await api(`/api/quality${query}`);
    if (mine !== asked) return;
    report = answer.report;
    failure = null;
    settleRemediation();
  } catch (e) {
    if (mine !== asked) return;
    report = null;
    failure = e.message;
  }
  renderQuality();
}

/// A reload the socket asked for. Same as the Refresh button: the report is
/// recomputed server-side on every read, so there is nothing to patch.
export function reloadQuality() {
  loadQuality();
}

export function wireQuality() {
  const refresh = $("qa-refresh");
  if (refresh) refresh.onclick = () => loadQuality();
  const seg = $("qa-view-seg");
  if (seg) {
    for (const b of seg.querySelectorAll("button[data-view]")) {
      b.onclick = () => setViewMode(b.dataset.view);
    }
  }
  const select = $("qa-scope-select");
  if (select) {
    select.onchange = () => {
      picked = select.value || null;
      openPair = null;
      renderQuality();
    };
  }
}

function setViewMode(mode) {
  const next = normalizeViewMode(mode);
  if (next === viewMode) return;
  viewMode = next;
  writeViewMode(viewMode);
  renderQuality();
}

// ------------------------------------------------------------------ render

export function renderQuality() {
  const note = $("qa-scope-note");
  if (note) {
    note.textContent = state.scope === null
      ? "No scope selected: every scope that binds a quality profile."
      : `${state.scope} and every scope below it that binds a quality profile.`;
  }
  const failed = $("qa-error");
  if (failed) {
    failed.textContent = failure || "";
    failed.hidden = !failure;
  }

  // Findings first and unconditionally: a profile that failed to parse, or
  // a config naming a profile with no file, is exactly why a report can
  // come back with no scopes -- the empty state must not hide the reason.
  renderFindings(report ? report.findings : []);

  const empty = $("qa-empty");
  const body = $("qa-body");
  const count = $("qa-count");
  if (!report || !hasProfiles(report)) {
    if (empty) empty.hidden = !report; // a fetch error already says why
    if (body) body.hidden = true;
    if (count) count.textContent = "";
    return;
  }
  if (empty) empty.hidden = true;
  if (body) body.hidden = false;
  if (count) count.textContent = `${report.scopes.length} scope${report.scopes.length === 1 ? "" : "s"}`;

  picked = focusScope(report, picked, state.scope);
  const scopeQuality = report.scopes.find((s) => s.scope === picked) || null;

  renderSegControl();
  renderScopePicker();
  renderHeatmap();
  renderTree(scopeQuality);
  renderTradeoffs(scopeQuality);
  renderGrid(scopeQuality);
  renderTable();
}

function renderSegControl() {
  const seg = $("qa-view-seg");
  if (seg) {
    for (const b of seg.querySelectorAll("button[data-view]")) {
      const on = b.dataset.view === viewMode;
      b.classList.toggle("on", on);
      b.setAttribute("aria-selected", on ? "true" : "false");
    }
  }
  for (const name of ["heatmap", "tree", "tradeoffs", "grid", "table"]) {
    const section = $(`qa-view-${name}`);
    if (section) section.hidden = name !== viewMode;
  }
}

/// Shown only for the three views that are about one scope -- on the
/// heatmap and the table every scope is already on screen.
function renderScopePicker() {
  const wrap = $("qa-scope-pick");
  if (wrap) wrap.hidden = !["tree", "tradeoffs", "grid"].includes(viewMode);
  const select = $("qa-scope-select");
  if (!select) return;
  select.innerHTML = report.scopes
    .map((s) => `<option value="${esc(s.scope)}"${s.scope === picked ? " selected" : ""}>${esc(s.scope)}</option>`)
    .join("");
}

/// Jump from anywhere to one scope's tree, optionally bringing one
/// characteristic into view.
function openTree(scope, characteristic) {
  picked = scope;
  spotlight = characteristic || null;
  openPair = null;
  viewMode = "tree";
  writeViewMode(viewMode);
  renderQuality();
}

function statusChip(status) {
  return `<span class="badge s-${esc(status)}">${esc(statusLabel(status))}</span>`;
}

function idChip(a) {
  return `<span class="qa-id-tag" title="importance ${esc(levelWord(a.importance))}, difficulty ${esc(levelWord(a.difficulty))}">${esc(idTag(a))}</span>`;
}

// ----------------------------------------------------------------- heatmap

/// A `<table>`, not an SVG: rows and columns are what it is, a screen
/// reader walks one natively, and a cell is a real `<button>`. The status
/// colour is the fill, importance the border weight (H thick, L hairline),
/// and an undeclared cell has neither -- a faint dot, not a colour, so it
/// can never be read as failing. Every cell also says its status in text,
/// so nothing here is colour alone.
function renderHeatmap() {
  const el = $("qa-heatmap");
  if (!el) return;
  const { columns, rows } = heatmapRows(report);
  const head = `<tr><th scope="col" class="qa-hm-corner">Scope</th>${columns
    .map((c) => `<th scope="col" class="qa-hm-col"><span>${esc(c.title)}</span></th>`)
    .join("")}</tr>`;
  const body = rows
    .map((r) => `<tr>
      <th scope="row" class="qa-hm-scope"><button type="button" class="linklike" data-qa-tree="${esc(r.scope)}">${esc(r.scope)}</button>
        <div class="sub">${r.profiles.map(esc).join(", ")}</div></th>
      ${r.cells
        .map((cell, i) => {
          const label = cellLabel(r.scope, columns[i], cell);
          if (!cell.declared) {
            // Text, not `aria-label`: a label on a plain `<span>` is not
            // reliably announced, and hidden text always is.
            return `<td class="qa-hm-cell"><span class="qa-hm-blank" title="${esc(label)}"><span aria-hidden="true">·</span><span class="vh">${esc(label)}</span></span></td>`;
          }
          return `<td class="qa-hm-cell"><button type="button" class="qa-hm-btn qa-st-${esc(cell.status)} qa-imp-${esc(cell.importance)}"
            data-qa-tree="${esc(r.scope)}" data-qa-char="${esc(cell.characteristic)}" title="${esc(label)}" aria-label="${esc(label)}">
            <span class="qa-hm-status">${esc(statusLabel(cell.status))}</span><span class="qa-hm-imp">${esc(cell.importance)}</span>
          </button></td>`;
        })
        .join("")}
    </tr>`)
    .join("");
  // The explanation sits outside the scrolling box: as a `<caption>` it
  // scrolled away with the columns on a phone and was cut off mid-word.
  el.innerHTML = `<p class="sub qa-hm-caption" id="qa-hm-caption">Worst scenario status per characteristic; border weight is importance.
    A blank cell is not declared -- not a stated concern, never failing.</p>
    <div class="qa-scroll"><table class="qa-heatmap" aria-describedby="qa-hm-caption">
    <thead>${head}</thead><tbody>${body}</tbody></table></div>`;
  for (const b of el.querySelectorAll("[data-qa-tree]")) {
    b.onclick = () => openTree(b.dataset.qaTree, b.dataset.qaChar);
  }
  const legend = $("qa-legend");
  if (legend) legend.innerHTML = legendHtml();
}

function legendHtml() {
  const swatch = (s) => `<span class="qa-leg-item"><span class="qa-leg-sw qa-st-${s} qa-imp-M"></span>${esc(statusLabel(s))}</span>`;
  return `${["met", "not_met", "stale", "no_data", "draft"].map(swatch).join("")}
    <span class="qa-leg-item"><span class="qa-leg-sw qa-leg-blank">·</span>not declared</span>
    <span class="qa-leg-item"><span class="qa-leg-sw qa-leg-weight qa-imp-H"></span>H</span>
    <span class="qa-leg-item"><span class="qa-leg-sw qa-leg-weight qa-imp-M"></span>M</span>
    <span class="qa-leg-item"><span class="qa-leg-sw qa-leg-weight qa-imp-L"></span>L importance</span>`;
}

// ------------------------------------------------------------ utility tree

function renderTree(scopeQuality) {
  const el = $("qa-tree");
  if (!el) return;
  if (!scopeQuality) { el.innerHTML = ""; return; }
  const tree = utilityTree(scopeQuality, report.catalogue);
  // A heatmap click asks to see a characteristic; it cannot stay closed.
  if (spotlight) collapsed.delete(nodeKey(scopeQuality.scope, "char", spotlight));
  el.innerHTML = `<div class="qa-tree-head">
      <h3>${esc(scopeQuality.scope)}</h3>
      <span class="sub">profiles: ${scopeQuality.profiles.map(esc).join(", ")}</span>
    </div>
    ${tree.length ? tree.map((g) => characteristicHtml(scopeQuality, g)).join("") : `<div class="empty">No attributes declared.</div>`}`;
  wireRemediate(el);
  for (const d of el.querySelectorAll("details[data-qa-node]")) {
    d.ontoggle = () => noteToggle(d.dataset.qaNode, d.open);
  }
  if (spotlight) {
    const target = el.querySelector(`[data-qa-group="${cssEscape(spotlight)}"]`);
    if (target && typeof target.scrollIntoView === "function") {
      // A frame later, once the view just un-hidden has a layout to scroll
      // to -- called straight away, the scroll starts and stalls near the top.
      const go = () => target.scrollIntoView({ block: "start", behavior: reducedMotion() ? "auto" : "smooth" });
      if (typeof requestAnimationFrame === "function") requestAnimationFrame(go); else go();
    }
    spotlight = null;
  }
}

/// The one motion this tab makes is a scroll; CSS cannot reach a
/// `scrollIntoView` option, so this asks the same media query the
/// stylesheet's own `prefers-reduced-motion` block answers.
function reducedMotion() {
  return typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
}

function cssEscape(s) {
  return typeof CSS !== "undefined" && CSS.escape ? CSS.escape(s) : String(s).replace(/["\\]/g, "\\$&");
}

/// `<details>` twice over -- a characteristic, then an attribute -- is the
/// whole collapsible tree: native and keyboard-operable. Open unless a
/// person closed it (`collapsed`, restored on every redraw). The
/// spotlighted characteristic (a heatmap click) is marked so the CSS can
/// ring it.
function characteristicHtml(scopeQuality, g) {
  const lit = spotlight === g.characteristic ? " qa-spotlight" : "";
  const key = nodeKey(scopeQuality.scope, "char", g.characteristic);
  return `<details class="qa-node qa-char${lit}"${collapsed.has(key) ? "" : " open"} data-qa-node="${esc(key)}" data-qa-group="${esc(g.characteristic)}">
    <summary><span class="qa-node-title">${esc(g.title)}</span>${statusChip(g.status)}</summary>
    ${g.attributes.map((a) => attributeHtml(scopeQuality, a)).join("")}
  </details>`;
}

function attributeHtml(scopeQuality, a) {
  const from = a.declared_at ? `from ${a.declared_at.profile} at ${a.declared_at.scope}` : "";
  const ai = a.standard === "iso-25059" ? ` <span class="qa-pack" title="ISO/IEC 25059 AI pack">AI</span>` : "";
  const key = nodeKey(scopeQuality.scope, "attr", a.id);
  return `<details class="qa-node qa-attr"${collapsed.has(key) ? "" : " open"} data-qa-node="${esc(key)}">
    <summary>
      <span class="qa-node-title">${esc(a.title)}${ai}</span>
      <code class="id">${esc(a.id)}</code>
      ${idChip(a)}
      ${statusChip(a.status)}
      <span class="sub qa-from">${esc(from)}</span>
    </summary>
    <div class="qa-scenarios">${(a.scenarios || []).length
      ? a.scenarios.map((s) => scenarioCardHtml(scopeQuality, a, s)).join("")
      : `<div class="empty">No scenarios yet -- a stated concern with no specification.</div>`}</div>
  </details>`;
}

function sentenceHtml(s) {
  return scenarioSentence(s)
    .map((seg) => (seg.part ? `<em class="qa-part qa-part-${esc(seg.part)}" title="${esc(seg.part)}">${esc(seg.text)}</em>` : esc(seg.text)))
    .join("");
}

/// One scenario: the six parts as a sentence, the verdict with its
/// reasons, then the evidence -- a bullet chart of the value against its
/// bound(s) and the metric's sparkline on the same value axis for a
/// continual measure, links to the task/run for a triggered one -- and the
/// remediation cell. The head is a `<div>`, not a `<header>`: the page's
/// bare `header` rule (its title bar) would otherwise paint a bar inside the
/// card -- the same trap `index.html`'s `.goal-header` comment names.
function scenarioCardHtml(scopeQuality, a, s) {
  const kind = measureKind(s.measure);
  const history = s.measure && s.measure.metric ? seriesPoints(report, s.measure.metric) : null;
  const refs = refLinks(scopeQuality.scope, s.refs);
  return `<article class="qa-card qa-card-${esc(s.status)}">
    <div class="qa-card-head">
      <span class="qa-card-id">${esc(s.id)}</span>
      ${s.kind ? `<span class="qa-kind">${esc(s.kind)}</span>` : ""}
      ${kind ? `<span class="qa-kind" title="${kind === "continual" ? "a threshold on a registry metric" : "a policy-style check"}">${esc(kind)}</span>` : ""}
      <span class="sp"></span>
      ${statusChip(s.status)}
    </div>
    <p class="qa-sentence">${sentenceHtml(s)}</p>
    ${bulletHtml(s, history)}
    ${refs.length ? `<div class="qa-refs">${refs.map((l) => `<a href="${esc(l.href)}">${esc(l.label)} <code class="id">${esc(l.id.slice(0, 8))}</code></a>`).join("")}</div>` : ""}
    ${(s.reasons || []).length ? `<ul class="qa-reasons">${s.reasons.map((r) => `<li>${esc(r)}</li>`).join("")}</ul>` : ""}
    ${remediateCellHtml(scopeQuality, a, s)}
  </article>`;
}

/// Inline SVG, sized by `viewBox` and stretched to the card's width. The
/// pass region is the wash, the value is the solid bar (coloured by the
/// verdict), each bound is a tick -- and the sparkline below reads the same
/// `valueDomain`, with the bound as a dashed line through it.
function bulletHtml(s, history) {
  const values = history ? history.map((p) => p.value) : null;
  const g = bulletGeometry(s.measure, s.value, values);
  if (!g) return "";
  const [lo, hi] = g.domain;
  const bounds = g.ticks.map((t) => `${t.kind === "above" ? "≥" : "≤"} ${formatNumber(t.value)}`).join(" and ");
  // A measure with no bound is a `missing_threshold` finding; say nothing
  // is required rather than "required ," with nothing after it.
  const required = bounds ? ` · required ${bounds}` : " · no threshold";
  const aria = `${s.measure.metric}: ${formatNumber(s.value)}${bounds ? `, required ${bounds}` : ", no threshold"}, axis ${formatNumber(lo)} to ${formatNumber(hi)}`;
  const bullet = `<svg class="qa-bullet" viewBox="0 0 ${g.width} ${g.height}" preserveAspectRatio="none" role="img" aria-label="${esc(aria)}">
      <rect class="qa-bullet-track" x="0" y="0" width="${g.width}" height="${g.height}"></rect>
      <rect class="qa-bullet-pass" x="${g.pass.x}" y="0" width="${g.pass.w}" height="${g.height}"></rect>
      <rect class="qa-bullet-bar qa-fill-${esc(s.status)}" x="${g.bar.x}" y="${g.bar.y}" width="${g.bar.w}" height="${g.bar.h}"></rect>
      ${g.ticks.map((t) => `<line class="qa-bullet-tick" x1="${t.x}" x2="${t.x}" y1="2" y2="${g.height - 2}"></line>`).join("")}
    </svg>`;
  const spark = sparkGeometry(history, valueDomain(s.measure, s.value, values), { width: g.width, height: 30 });
  // One polyline per unbroken run of days; a lone day is a zero-length line
  // whose round cap draws a dot, since a `<circle>` would be stretched by
  // `preserveAspectRatio="none"`.
  const runs = spark
    ? spark.segments
      .map((seg) => (seg.length > 1
        ? `<polyline class="qa-spark-line" points="${seg.join(" ")}"></polyline>`
        : `<line class="qa-spark-line qa-spark-dot" x1="${seg[0].split(",")[0]}" y1="${seg[0].split(",")[1]}" x2="${seg[0].split(",")[0]}" y2="${seg[0].split(",")[1]}"></line>`))
      .join("")
    : "";
  const sparkSvg = spark
    ? `<svg class="qa-spark" viewBox="0 0 ${spark.width} ${spark.height}" preserveAspectRatio="none" aria-hidden="true">
        ${g.ticks.map((t) => `<line class="qa-spark-bound" x1="0" x2="${spark.width}" y1="${spark.y(t.value).toFixed(1)}" y2="${spark.y(t.value).toFixed(1)}"></line>`).join("")}
        ${runs}
      </svg>
      <div class="qa-axis sub"><span>history: ${spark.count} points over ${spark.days} days</span></div>`
    : "";
  return `<div class="qa-evidence">
    <div class="qa-axis sub"><span>${esc(formatNumber(lo))}</span><span>actual <b>${esc(formatNumber(s.value))}</b>${esc(required)}</span><span>${esc(formatNumber(hi))}</span></div>
    ${bullet}
    ${sparkSvg}
  </div>`;
}

// -------------------------------------------------------------- remediate
//
// "Create task" asks before it acts -- inline, never `confirm()` (the same
// restraint `policy.js` states for its own button) -- and shows the task
// already open for the scenario instead of offering to make another
// (`#98`, from `ScopeQuality::open_tasks`). The daemon remains the guard: a
// refusal (already met, or a `no_data` gap no task can close) lands as
// inline text naming its reason. Every step lives in `remediation`, never
// only in the DOM, and the cell is drawn from it -- so a reload mid-way
// redraws the same confirmation, the same "creating…", the same refusal.

function remediateCellHtml(scopeQuality, a, s) {
  const key = remediationKey(scopeQuality.scope, a.id, s.id);
  const entry = remediation.get(key);
  if (entry && entry.phase === "done") {
    return `<div class="qa-remediate"><a class="qa-task-link" href="${esc(taskHref(scopeQuality.scope, entry.task.id))}" title="${esc(entry.task.title)}">${entry.created ? "Task created →" : "Task open →"}</a></div>`;
  }
  const open = openTaskFor(scopeQuality, a.id, s.id);
  if (open) {
    return `<div class="qa-remediate"><a class="qa-task-link" href="${esc(taskHref(scopeQuality.scope, open))}">Task open →</a></div>`;
  }
  if (!canRemediate(s, report.findings)) return "";
  const cell = (inner) => `<div class="qa-remediate" data-qa-remediate-cell="${esc(key)}" data-scenario="${esc(s.id)}">${inner}</div>`;
  if (entry && (entry.phase === "confirming" || entry.phase === "pending")) {
    const busy = entry.phase === "pending" ? " disabled" : "";
    return cell(`<span class="sub">Create a task to meet <code class="id">${esc(a.id)}/${esc(s.id)}</code> in ${esc(scopeQuality.scope)}?</span>
      <button type="button" class="btn primary" data-qa-confirm${busy}>${entry.phase === "pending" ? "Creating…" : "Create task"}</button>
      <button type="button" class="btn" data-qa-cancel${busy}>Cancel</button>`);
  }
  const error = entry && entry.phase === "error" ? `<div class="sub qa-remediate-err">${esc(entry.message)}</div>` : "";
  return cell(`<button type="button" class="btn" data-qa-remediate>Create task</button>${error}`);
}

function wireRemediate(root) {
  for (const cell of root.querySelectorAll("[data-qa-remediate-cell]")) {
    const key = cell.dataset.qaRemediateCell;
    const on = (sel, fn) => { const b = cell.querySelector(sel); if (b) b.onclick = () => fn(key); };
    on("[data-qa-remediate]", askRemediate);
    on("[data-qa-confirm]", confirmRemediate);
    on("[data-qa-cancel]", cancelRemediate);
  }
}

/// Redraw after a remediation step. The tree is the only view that shows
/// the cell, and a redraw restores everything else it held (`collapsed`).
function redrawTree() {
  if (!report || !hasProfiles(report)) return;
  renderTree(report.scopes.find((s) => s.scope === picked) || null);
}

export function askRemediate(key) {
  remediation.set(key, { phase: "confirming" });
  redrawTree();
}

export function cancelRemediate(key) {
  remediation.delete(key);
  redrawTree();
}

export async function confirmRemediate(key) {
  const current = remediation.get(key);
  if (current && current.phase === "pending") return;
  const [scope, attribute, scenario] = JSON.parse(key);
  remediation.set(key, { phase: "pending" });
  redrawTree();
  try {
    const answer = await api("/api/quality/remediate", {
      method: "POST",
      body: JSON.stringify(remediateBody(scope, attribute, scenario)),
    });
    remediation.set(key, { phase: "done", task: answer.result.task, created: answer.result.created });
  } catch (e) {
    remediation.set(key, { phase: "error", message: e.message });
  }
  // Drawn from state into whatever tree is on screen *now* -- a reload
  // while the request was out replaced the cell it started from.
  redrawTree();
}

/// After a fresh report: a finished remediation whose task the report now
/// lists under `open_tasks` has nothing left to say that the report does
/// not, so it is dropped; anything in progress or refused is kept.
function settleRemediation() {
  for (const [key, entry] of remediation) {
    if (entry.phase !== "done") continue;
    const [scope, attribute, scenario] = JSON.parse(key);
    const sq = report.scopes && report.scopes.find((s) => s.scope === scope);
    if (sq && openTaskFor(sq, attribute, scenario)) remediation.delete(key);
  }
}

/// A `<details>` opened or closed by a person (its `toggle` event).
export function noteToggle(key, open) {
  if (open) collapsed.delete(key); else collapsed.add(key);
}

// ---------------------------------------------------------------- tradeoffs

/// A numbered grid: the rows carry the attribute names, the columns only
/// their numbers -- seven long ids across the top would not fit a phone,
/// and the row legend already says which number is which. A marked cell is
/// a button; the detail opens below the grid rather than in a modal, so
/// the grid stays in view while the point is read.
function renderTradeoffs(scopeQuality) {
  const el = $("qa-tradeoffs");
  if (!el) return;
  if (!scopeQuality) { el.innerHTML = ""; return; }
  const m = tradeoffMatrix(scopeQuality);
  if (!m.count) {
    el.innerHTML = `<div class="empty">No trade-off points declared for ${esc(scopeQuality.scope)}. A profile's
      <code class="id">tradeoffs:</code> names two attributes, the point where improving one costs the other, and the decision behind it.</div>`;
    return;
  }
  if (openPair && !m.cells.has(openPair)) openPair = null;
  const head = `<tr><th scope="col"></th>${m.axes.map((_, j) => `<th scope="col" class="qa-tm-num">${j + 1}</th>`).join("")}</tr>`;
  const rows = m.axes
    .map((row, i) => `<tr>
      <th scope="row" class="qa-tm-row${row.declared ? "" : " qa-undeclared"}"><span class="qa-tm-n">${i + 1}</span>${esc(attributeLabel(row.id, report.catalogue))}
        ${row.declared ? "" : `<span class="sub"> (not declared)</span>`}</th>
      ${m.axes
        .map((col, j) => {
          const key = `${row.id}|${col.id}`;
          const list = m.cells.get(key);
          // The diagonal is an attribute against itself: empty by
          // definition, except when a profile declared exactly that (a
          // `bad_tradeoff` finding, kept by the loader) -- which still gets
          // a button, so the point is readable rather than hidden.
          if (i === j && !list) return `<td class="qa-tm-cell qa-tm-diag" aria-hidden="true"></td>`;
          if (!list) return `<td class="qa-tm-cell"></td>`;
          const on = openPair === key || openPair === `${col.id}|${row.id}`;
          return `<td class="qa-tm-cell"><button type="button" class="qa-tm-btn${on ? " on" : ""}" data-qa-pair="${esc(key)}"
            aria-label="${esc(`${row.id} and ${col.id}: ${list.length} trade-off point${list.length === 1 ? "" : "s"}`)}">${list.length}</button></td>`;
        })
        .join("")}
    </tr>`)
    .join("");
  el.innerHTML = `<div class="qa-scroll"><table class="qa-tradeoff-matrix"><thead>${head}</thead><tbody>${rows}</tbody></table></div>
    <div class="qa-tm-detail" id="qa-tradeoff-detail" aria-live="polite">${pairDetailHtml(scopeQuality, m)}</div>`;
  for (const b of el.querySelectorAll("[data-qa-pair]")) {
    b.onclick = () => {
      openPair = b.dataset.qaPair;
      renderTradeoffs(scopeQuality);
    };
  }
}

function pairDetailHtml(scopeQuality, m) {
  if (!openPair) {
    return `<p class="sub">Select a numbered cell to read the trade-off point, its rationale and the decision behind it.</p>`;
  }
  const list = m.cells.get(openPair) || [];
  return list
    .map((t) => {
      const link = decisionLink(t.decision, scopeQuality.scope);
      const decision = !link
        ? `<span class="sub">no decision linked</span>`
        : link.kind === "knowledge"
          ? `<a href="${esc(link.href)}">Decision: ${esc(link.label)} (Knowledge) →</a>`
          : link.kind === "external"
            ? `<a href="${esc(link.href)}" target="_blank" rel="noopener">Decision: ${esc(link.label)} ↗</a>`
            : `<span>Decision: ${esc(link.label)}</span>`;
      return `<div class="qa-tm-point">
        <div class="qa-tm-between">${t.between.map((id) => esc(attributeLabel(id, report.catalogue))).join(" ⇄ ")}</div>
        <p>${esc(t.point)}</p>
        <div class="qa-tm-links">${decision}
          ${t.declared_at ? `<span class="sub">declared in ${esc(t.declared_at.profile)} at ${esc(t.declared_at.scope)}</span>` : ""}</div>
      </div>`;
    })
    .join("");
}

// ------------------------------------------------- importance × difficulty

/// A real `<table>`: importance down the rows (H at the top), difficulty
/// across the columns (L on the left), so a screen reader announces each
/// cell's two headers -- the grid it looks like is what it is.
function renderGrid(scopeQuality) {
  const el = $("qa-grid");
  if (!el) return;
  if (!scopeQuality) { el.innerHTML = ""; return; }
  const cells = importanceDifficultyGrid(scopeQuality);
  const cellHtml = (c) => `<td class="qa-grid-cell${c.hot ? " qa-hot" : ""}">
      <div class="qa-grid-tag">${esc(`(${c.importance},${c.difficulty})`)}${c.hot ? ` <span class="qa-hot-note">measure these first</span>` : ""}</div>
      ${c.attributes
        .map((a) => `<button type="button" class="qa-grid-attr qa-st-${esc(a.status)}" data-qa-attr-char="${esc(a.characteristic)}">
          ${esc(attributeLabel(a.id, report.catalogue))}<span class="qa-grid-status">${esc(statusLabel(a.status))}</span></button>`)
        .join("")}
    </td>`;
  const rows = LEVELS.map((imp) => `<tr><th scope="row" class="qa-grid-rowlabel"><span class="vh">importance </span>${esc(imp)}</th>${cells
    .filter((c) => c.importance === imp)
    .map(cellHtml)
    .join("")}</tr>`).join("");
  el.innerHTML = `<table class="qa-grid">
      <caption class="qa-grid-caption">Importance ↓ by difficulty → <span class="vh">for ${esc(scopeQuality.scope)}</span></caption>
      <thead><tr><td class="qa-grid-corner"></td>${["L", "M", "H"]
        .map((d) => `<th scope="col" class="qa-grid-collabel"><span class="vh">difficulty </span>${d}</th>`).join("")}</tr></thead>
      <tbody>${rows}</tbody>
    </table>`;
  for (const b of el.querySelectorAll("[data-qa-attr-char]")) {
    b.onclick = () => openTree(scopeQuality.scope, b.dataset.qaAttrChar);
  }
}

// -------------------------------------------------------------------- table

function renderTable() {
  const body = $("qa-table");
  if (!body) return;
  body.innerHTML = tableRows(report)
    .map((r) => `<tr>
      <td>${esc(r.scope)}</td>
      <td><div class="title">${esc(r.label)}</div><code class="id">${esc(r.attribute)}</code></td>
      <td>${esc(`(${r.importance},${r.difficulty})`)}</td>
      <td>${r.scenario ? `<code class="id">${esc(r.scenario)}</code>${r.kind ? `<div class="sub">${esc(r.kind)}</div>` : ""}` : `<span class="sub">none</span>`}</td>
      <td>${statusChip(r.status)}</td>
      <td class="sub">${esc(r.measure || "no response measure")}</td>
      <td>${r.value === null ? `<span class="sub">—</span>` : esc(formatNumber(r.value))}</td>
      <td class="sub">${r.reasons.map(esc).join("; ")}</td>
    </tr>`)
    .join("");
}

// ------------------------------------------------------------------ findings

function renderFindings(findings) {
  const el = $("qa-findings");
  const none = $("qa-no-findings");
  const groups = findingsByKind(findings);
  if (el) {
    el.innerHTML = [...groups.entries()]
      .map(([kind, rows]) => `<div class="pol-finding-group">
        <h4>${esc(findingLabel(kind))}</h4>
        <ul>${rows.map((f) => `<li><code>${esc(f.subject)}</code> — ${esc(f.detail)}</li>`).join("")}</ul>
      </div>`)
      .join("");
  }
  if (none) none.hidden = groups.size !== 0 || !report;
}
