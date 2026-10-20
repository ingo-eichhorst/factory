//! The L5 Improvement Metrics tab (`#333`): every metric the selected scope
//! is measured by -- its value or `None` and why, how fresh it is against the
//! strictest consumer's `max_age`, where it comes from, and every policy
//! control, quality scenario and key result that reads it, with each one's
//! own verdict. `metrics-model.js` holds every pure shaping function this
//! file draws from; this file only fetches, draws and wires clicks.
//!
//! No poll: one `GET /api/metrics?scope=` per load, recomputed fresh by the
//! daemon (a service-backed source is asked once per load, never cached),
//! and `app.js` refetches when the rail selection changes.

import { $, api, esc, state } from "./core.js";
import { routeHref } from "./scopes.js";
import {
  betterLabel,
  consumerBounds,
  consumerName,
  countLine,
  dependencyTarget,
  filterRows,
  formatValue,
  freshnessClass,
  freshnessLabel,
  listingScope,
  metricsUrl,
  shapeRows,
  sourceCell,
  statusLabel,
} from "./metrics-model.js";

let asked = 0;
let answer = null;
let failure = null;
let listed = null;
let query = "";

export async function loadMetrics() {
  const mine = ++asked;
  const scope = listingScope(state.scope, state.scopes, state.root);
  listed = scope;
  if (!scope) {
    answer = null;
    failure = null;
    renderMetrics();
    return;
  }
  try {
    const got = await api(metricsUrl(scope));
    if (mine !== asked) return;
    answer = got;
    failure = null;
  } catch (e) {
    if (mine !== asked) return;
    answer = null;
    failure = e.message;
  }
  renderMetrics();
}

export function wireMetrics() {
  const refresh = $("metrics-refresh");
  if (refresh) refresh.onclick = () => void loadMetrics();
  const q = $("metrics-query");
  if (q) q.oninput = () => { query = q.value; renderMetrics(); };
}

function chip(cls, label) {
  return `<span class="badge s-${esc(cls)}">${esc(label)}</span>`;
}

function sourceHtml(source) {
  const cell = sourceCell(source);
  const target = dependencyTarget(source);
  const label = target
    ? `<a href="${esc(routeHref(target.scope, "dependencies"))}" data-dep-scope="${esc(target.scope)}" data-dep-service="${esc(target.service)}">${esc(cell.label)}</a>`
    : esc(cell.label);
  return `<div class="${cell.bad ? "bad" : ""}">${label}</div>${cell.detail ? `<div class="sub">${esc(cell.detail)}</div>` : ""}`;
}

function consumersHtml(consumers) {
  if (!consumers.length) return `<span class="sub">declared by the scope; nothing reads it yet</span>`;
  return consumers
    .map((c) => {
      const bounds = consumerBounds(c);
      const why = (c.reasons || []).join("; ");
      return `<div class="metric-consumer" title="${esc(why)}">${chip(c.status, statusLabel(c.status))} ${esc(consumerName(c))}${bounds ? ` <span class="sub">${esc(bounds)}</span>` : ""}</div>`;
    })
    .join("");
}

function rowHtml(row) {
  const value = formatValue(row.value, row.unit);
  const asOf = row.asOf ? new Date(row.asOf).toLocaleString() : "";
  return `<tr>
    <td><strong>${esc(row.title)}</strong><div class="sub"><code>${esc(row.id)}</code></div><div class="sub">${esc([row.unit, betterLabel(row.better)].filter(Boolean).join(" · "))}</div></td>
    <td><strong>${esc(value)}</strong>${row.value === null && row.reason ? `<div class="sub">${esc(row.reason)}</div>` : ""}</td>
    <td>${chip(freshnessClass(row.freshness), freshnessLabel(row.freshness))}${row.maxAge ? ` <span class="sub">max_age ${esc(row.maxAge)}</span>` : ""}${asOf ? `<div class="sub">as of ${esc(asOf)}</div>` : ""}</td>
    <td>${sourceHtml(row.source)}</td>
    <td>${consumersHtml(row.consumers)}</td>
  </tr>`;
}

export function renderMetrics() {
  const note = $("metrics-note");
  const error = $("metrics-error");
  const body = $("metrics-rows");
  const empty = $("metrics-empty");
  const count = $("metrics-count");
  const findings = $("metrics-findings");
  if (!body) return;
  if (error) {
    error.textContent = failure || "";
    error.hidden = !failure;
  }
  if (!listed) {
    if (note) note.textContent = "No scope sits on the instance root. Select one scope in the rail.";
    body.innerHTML = "";
    if (count) count.textContent = "";
    if (empty) empty.hidden = true;
    if (findings) findings.innerHTML = "";
    return;
  }
  if (note) {
    // Key results are the instance root's own, so only its listing has them.
    const atRoot = listed === listingScope(null, state.scopes, state.root);
    note.textContent = `Scope ${listed} and everything below it: what its applicable controls, bound quality scenarios${atRoot ? ", current key results" : ""} and its own declarations read.`;
  }
  const rows = filterRows(shapeRows(answer), query);
  if (count) count.textContent = answer ? countLine(rows) : "";
  body.innerHTML = rows.map(rowHtml).join("");
  if (empty) empty.hidden = !answer || rows.length !== 0;
  if (findings) {
    const list = answer?.findings || [];
    findings.innerHTML = list.length
      ? `<h3 class="dep-heading">scope.metrics findings</h3>${list
          .map((f) => `<div class="env-note bad"><code>${esc(f.subject)}</code> ${esc(f.detail)}</div>`)
          .join("")}`
      : "";
  }
  for (const link of body.querySelectorAll("a[data-dep-service]")) {
    link.onclick = () => {
      state.dependenciesSegment = "services";
      state.dependenciesFocus = link.dataset.depService;
    };
  }
}
