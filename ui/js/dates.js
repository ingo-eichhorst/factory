//! People-side composition: the Infrastructure location is navigation, not
//! permission for L1 to read L2/L4/L6. All surfaces share the read-only API.
import { $, api, esc, state } from "./core.js";
import { datesQuery, datesSummary, expiryText, stateText, dateTone, dependencyText, dependencyWarnings } from "./dates-model.js";

let asked = 0;
let badgeAsked = 0;
let badgeReport = null;

export async function fetchDates(scope = null) {
  try { return (await api(datesQuery(scope))).report; } catch { return null; }
}

export async function loadDateBadges() {
  const mine = ++badgeAsked;
  const report = await fetchDates();
  if (mine === badgeAsked) badgeReport = report;
}

export function dateBadges(dependency) {
  const entries = dependencyWarnings(badgeReport, dependency);
  return entries.map(entry => `<a class="tag sys-badge" data-tone="${dateTone(entry)}" href="${esc(entry.href)}" title="${esc(`${entry.observation.name}: ${expiryText(entry)} · ${entry.observation.renew}`)}">${esc(entry.observation.name)} · ${esc(stateText(entry))}${entry.milestone === "scheduled_run" ? " · scheduled risk" : ""}</a>`).join(" ");
}

export async function refreshDates() {
  const mine = ++asked;
  const scope = state.scope;
  const report = await fetchDates(scope);
  if (mine !== asked || scope !== state.scope) return;
  state.dates = report;
  renderDates();
}

export function dateRow(entry) {
  const item = entry.observation;
  const declared = entry.conflict ? `<p class="env-note warn">Conflicting declaration: ${esc(entry.declared_no_expiry ? "no expiry" : entry.declared_expires_at)}. Observed metadata wins.</p>` : "";
  const risk = (entry.scheduled_risks || []).map(run => `<li>${esc(`${run.scope}/${run.agent}: ${run.title} at ${run.next_run_at}`)}</li>`).join("");
  return `<article class="infra-card dates-entry" data-tone="${dateTone(entry)}">
    <header class="infra-card-head"><h3>${esc(item.name)}</h3><span class="tag sys-badge" data-tone="${dateTone(entry)}">${esc(stateText(entry))}</span></header>
    <p><b>${esc(expiryText(entry))}</b> · ${esc(item.kind)} · ${esc(item.basis)} · ${esc(item.source)}</p>
    <p class="sub">${esc(item.detail)}${item.observed_at ? ` · observed ${esc(item.observed_at)}` : ""}</p>
    ${item.issue ? `<p class="env-note warn">${esc(item.issue)}${item.expires_at ? " · expiry shown is last known, not a successful fresh observation" : ""}</p>` : ""}${declared}
    <dl class="infra-facts"><div class="infra-fact"><dt>Depends</dt><dd>${esc(dependencyText(entry))}</dd></div>
    <div class="infra-fact"><dt>Renew</dt><dd>${esc(item.renew)}</dd></div>
    <div class="infra-fact"><dt>Owner</dt><dd>${esc(item.owner)}</dd></div>
    <div class="infra-fact"><dt>Lead</dt><dd>${esc(String(item.lead_seconds / 86400))} days</dd></div></dl>
    ${risk ? `<p class="env-note warn">Will lapse before a scheduled run. The warning does not block or change its schedule.</p><ul>${risk}</ul>` : ""}
    ${["policy_attestation", "cra_deadline"].includes(item.source) ? `<a href="${esc(entry.href)}">Open native policy / CRA clock</a>` : ""}
  </article>`;
}

export function renderDates() {
  const host = $("dates");
  if (!host) return;
  const report = state.dates;
  if (report === undefined) { host.innerHTML = `<p>Loading important dates…</p>`; return; }
  if (!report) { host.innerHTML = `<p class="err">Important dates are not available right now. No expiry state is inferred.</p>`; return; }
  const summary = datesSummary(report);
  const issues = (report.observation_issues || []).map(issue => `<li>${esc(issue)}</li>`).join("");
  host.innerHTML = `<p>${esc(summary.counts)} · next expiry: ${esc(summary.next)}</p>
    ${issues ? `<ul class="bk-warnings">${issues}</ul>` : ""}
    ${(report.entries || []).map(dateRow).join("") || `<p class="empty">No important dates known for this selection.</p>`}`;
}

export function wireDates() {
  const button = $("dates-refresh");
  if (button) button.onclick = refreshDates;
}
