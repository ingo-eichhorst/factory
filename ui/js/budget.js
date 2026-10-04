//! Read-only L6 authored limits plus the typed L4 spend port, via HTTP.
import { $, api, esc, state } from "./core.js";
import { GROUPS, budgetUrl, budgetCard, spendRows, usd, uncertainty, burnDown } from "./budget-model.js";

let asked = 0;
let group = "scope";
let refreshTimer = null;

function cardHtml(raw, month) {
  const c = budgetCard(raw);
  const chart = burnDown(raw, month);
  const progress = c.usedPercent == null ? "" : `<progress max="100" value="${c.usedPercent}" aria-label="Monthly budget used"></progress>`;
  const burn = chart ? `<svg class="budget-burn" viewBox="0 0 ${chart.width} ${chart.height}" role="img" aria-label="Observed remaining budget by UTC run-start day versus a linear monthly plan"><polyline class="budget-plan" points="${chart.planned}"/><polyline class="budget-observed" points="${chart.points}"/></svg><p class="sub">Solid: observed remaining (floored at $0); dashed: linear plan. Above-limit spend is shown in the verdict.</p>` : "";
  const ancestor = c.ancestorNote ? `<p class="env-note">${esc(c.ancestorNote)}</p>` : "";
  return `<article class="budget-card budget-${esc(c.assessment.state)}">
    <div class="budget-title"><h3>${esc(c.scope)}</h3><span>${esc(c.status)}</span></div>
    <p class="sub">${esc(c.id)} · ${esc(c.path)}</p>
    <p>Monthly limit: ${esc(c.limit)} · ${esc(usd(c.spent.cost_usd))} known</p>
    <p>${esc(c.assessment.reason)}</p>
    ${progress}
    <p>Remaining: ${esc(c.remaining)} · Month-end at current pace: ${esc(c.projected)}</p>
    ${burn}
    ${ancestor}
  </article>`;
}

function spendTableHtml(rows) {
  if (!rows.length) return '<p class="empty">No runs started in this month-to-date window.</p>';
  const body = rows.map(r => `<tr><td>${esc(r.name)}</td><td>${r.runs}</td><td>${esc(r.cost)}</td><td>${esc(r.tokenText)}</td><td>${esc(r.uncertainty) || "—"}</td></tr>`).join("");
  return `<table><thead><tr><th>Group</th><th>Runs</th><th>USD</th><th>Tokens</th><th>Uncertainty</th></tr></thead><tbody>${body}</tbody></table>`;
}

function render(report) {
  const month = report.month;
  $("budget-month").textContent = `${month.from.slice(0, 7)} UTC · as of ${month.as_of}`;
  $("budget-catalogue").textContent = `Authored limits: ${report.catalogue}. Missing means no limit; provider plan allowances are not budgets.`;
  $("budget-findings").textContent = report.findings.join(" · ");
  $("budget-findings").hidden = report.findings.length === 0;
  $("budget-cards").innerHTML = report.budgets.map(c => cardHtml(c, month)).join("") || '<p class="empty">No configured scopes.</p>';
  const total = report.spend.total;
  $("budget-spend-note").textContent = `${state.scope || "Whole instance"} · ${total.runs} runs · ${usd(total.cost_usd)} known. ${uncertainty(total, report.spend.unattributed_runs)}`;
  const rows = spendRows(report.spend);
  $("budget-spend").innerHTML = spendTableHtml(rows);
}

export async function loadBudget() {
  const ticket = ++asked;
  $("budget-error").hidden = true;
  // A failed/new scope read must not leave the previous scope's safe verdict.
  $("budget-body").hidden = true;
  $("budget-loading").hidden = false;
  try {
    const data = await api(budgetUrl(state.scope, group));
    if (ticket !== asked) return;
    render(data.report);
    $("budget-body").hidden = false;
  } catch (e) {
    if (ticket !== asked) return;
    $("budget-error").textContent = e.message;
    $("budget-error").hidden = false;
  } finally {
    if (ticket === asked) $("budget-loading").hidden = true;
  }
}

export function reloadBudget() {
  if (refreshTimer !== null) return;
  refreshTimer = setTimeout(() => {
    refreshTimer = null;
    if (state.tab === "budget") void loadBudget();
  }, 1500);
}

export function wireBudget() {
  $("budget-group").innerHTML = GROUPS.map(([id, name]) => `<option value="${id}">${name}</option>`).join("");
  $("budget-group").onchange = () => { group = $("budget-group").value; void loadBudget(); };
  $("budget-refresh").onclick = () => loadBudget();
}
