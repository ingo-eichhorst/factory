//! L1 Operations (#185): the environments the systems Factory builds run in.
//! One answer from `/api/environments`, drawn as a card per environment in
//! promotion order -- its status, what release runs there, uptime against
//! its SLO, error budget, each check's last day and the four DORA keys --
//! then the deployment timeline and the release catalogue.
//!
//! Not Factory's own production line: that is the L4 Line tab
//! (`operations.js`, `/api/operations`). Promote creates a policy-gated
//! release workflow; the deployment still waits for an owner approval.
//!
//! Every formatter lives in `environments-model.js`, which the Node tests
//! import; this file only puts their answers on screen.

import { $, api, esc, state } from "./core.js";
import {
  CONFIG_SNIPPET,
  MISSING,
  budgetLevel,
  budgetText,
  bucketTone,
  deploymentRows,
  doraRows,
  environmentsFailure,
  environmentsQuery,
  fmtAgo,
  fmtPct,
  incidentText,
  lastCheckText,
  promotionChoices,
  releaseRows,
  releaseText,
  sloText,
  statusLabel,
  statusText,
  statusTone,
  tierLabel,
  uptimeLevel,
} from "./environments-model.js";

// The rail can move while a request is in flight; only the newest answer
// is drawn.
let asked = 0;
const promoting = new Set();
let promotionNotice = null;

export async function promoteEnvironment(environment, deployment) {
  const card = state.environments?.environments?.find(card => card.name === environment);
  if (!card?.promotion_ready || card.current?.id !== deployment || promoting.has(environment)) return;
  const scope = state.scope;
  promoting.add(environment);
  renderEnvironments();
  try {
    const { run } = await api("/api/environments/promote", { method: "POST", body: JSON.stringify({ environment, deployment }) });
    promotionNotice = { scope, run };
  } catch (error) {
    promotionNotice = { scope, error: error.message };
  } finally {
    promoting.delete(environment);
    if (state.scope === scope) await refreshEnvironments();
  }
}

function promotionButton(choice) {
  return `<button type="button" data-environment-promote="${esc(choice.source)}" data-deployment="${esc(choice.deployment)}"${promoting.has(choice.source) ? " disabled" : ""}>Promote to ${esc(choice.target)}</button>`;
}

function cardPromotion(card) {
  if (!card.promotes_to) return "";
  let action;
  if (card.promotion_ready && card.current) {
    action = promotionButton({ source: card.name, target: card.promotes_to, deployment: card.current.id });
  } else {
    action = `<span class="sub">${esc(card.promotion_reason || "Promotion is not available on this daemon.")}</span>`;
  }
  return `<div class="sys-promotion">${action}<p class="sub">Creates a release workflow; deployment waits for owner approval.</p></div>`;
}

function promotionNoticeHTML() {
  if (!promotionNotice || promotionNotice.scope !== state.scope) return "";
  if (promotionNotice.error) return `<p class="sys-promotion-notice bk-bad" role="status">${esc(promotionNotice.error)}</p>`;
  const { run } = promotionNotice;
  const href = `#${encodeURIComponent(run.scope)}/proc/workflows/${encodeURIComponent(run.workflow_id)}/run/${encodeURIComponent(run.id)}`;
  return `<p class="sys-promotion-notice" role="status">Promotion created. <a href="${href}">Open release workflow</a>. Deployment waits for owner approval.</p>`;
}

// ------------------------------------------------------------------ fetching

/// Sets `state.environments`, or `state.environmentsError` with
/// `state.environmentsUnavailable` saying whether the failure was only a
/// daemon too old to serve the endpoint. Renders nothing.
export async function loadEnvironments() {
  const mine = ++asked;
  let report = null;
  let error = null;
  try {
    report = (await api(`/api/environments${environmentsQuery(state.scope)}`)).report;
  } catch (e) {
    error = e;
  }
  if (mine !== asked) return false;
  state.environments = report;
  state.environmentsUnavailable = !!error && environmentsFailure(error) === "unavailable";
  state.environmentsError = error && !state.environmentsUnavailable ? error.message : null;
  return true;
}

export async function refreshEnvironments() {
  const button = $("environments-refresh");
  if (button) button.disabled = true;
  try {
    if (await loadEnvironments()) renderEnvironments();
  } finally {
    if (button) button.disabled = false;
  }
}

// ------------------------------------------------------------------ pieces

function fact(label, value, level) {
  const v = value === MISSING ? `<span class="infra-missing">${MISSING}</span>` : esc(value);
  return `<div class="infra-fact"><dt>${esc(label)}</dt><dd${level && level !== "none" ? ` class="bk-${level}"` : ""}>${v}</dd></div>`;
}

function strip(check) {
  const cells = (check.strip || []).map(b => {
    const tone = bucketTone(b);
    const title = `${new Date(b.start).toISOString().slice(11, 16)} UTC · ${b.ok} ok (${b.slow || 0} slow), ${b.failed} failed`;
    return `<span class="sys-slot" data-tone="${tone}" title="${esc(title)}"></span>`;
  }).join("");
  return `<div class="sys-strip" role="img" aria-label="${esc(`${check.name}: the last 24 hours in half-hour slots`)}">${cells}</div>`;
}

function checks(card, now) {
  if (!card.checks.length) {
    return card.declared
      ? `<p class="sys-none">No checks declared: nothing says whether it is up.</p>`
      : `<p class="sys-none">Not declared in any scope's config: a deployment named it, so it has no checks or SLO.</p>`;
  }
  return `<div class="sys-checks">${card.checks.map(c => `
    <div class="sys-check">
      <div class="sys-check-head">
        <span class="sys-dot" data-tone="${c.last ? (c.last.ok ? (c.last.slow ? "warn" : "ok") : "bad") : "none"}"></span>
        <span class="sys-check-name">${esc(c.name)}</span>
        <span class="tag">${esc(c.kind)}</span>
        <span class="sub mono" title="${esc(c.target)}">${esc(c.target)}</span>
      </div>
      ${strip(c)}
      <div class="sub">${esc(lastCheckText(c, now))} · every ${esc(String(c.every_seconds))}s${c.slow_after_ms ? ` · slow above ${esc(String(c.slow_after_ms))}ms` : ""}</div>
    </div>`).join("")}</div>`;
}

function incidents(card, now) {
  if (!card.incidents.length) return "";
  const rows = card.incidents.slice(0, 5).map(i =>
    `<li class="${i.ended_at ? "" : "bk-bad"}">${esc(incidentText(i, now))}</li>`).join("");
  const more = card.incidents.length > 5 ? `<li class="sub">and ${card.incidents.length - 5} earlier</li>` : "";
  return `<div class="sys-incidents"><h4>Incidents <span class="sub">in the SLO window</span></h4><ul>${rows}${more}</ul></div>`;
}

function card(c, now) {
  const tone = statusTone(c.status);
  const current = c.current
    ? `${releaseText(c.current.release)} · ${fmtAgo(now, c.current.finished_at || c.current.started_at)}`
    : MISSING;
  const running = c.running
    ? `<p class="env-note warn">Deploying ${esc(releaseText(c.running.release))} since ${esc(fmtAgo(now, c.running.started_at))}.</p>`
    : "";
  const link = c.url ? `<a class="sub" href="${esc(c.url)}" target="_blank" rel="noopener">${esc(c.url)}</a>` : "";
  return `<article class="infra-card sys-card" data-tone="${tone}" aria-labelledby="sys-${esc(c.name)}">
    <header class="infra-card-head">
      <span class="sys-dot" data-tone="${tone}"></span>
      <h3 id="sys-${esc(c.name)}">${esc(c.name)}</h3>
      <span class="tag sys-badge" data-tone="${tone}">${esc(statusLabel(c.status))}</span>
      <span class="tag">${esc(tierLabel(c.tier))}</span>
      ${c.promotes_to ? `<span class="sub">promotes to ${esc(c.promotes_to)}</span>` : ""}
      <span class="sub">${esc(c.scope)}</span>
      ${link}
    </header>
    <p class="sys-status">${esc(statusText(c, now))}</p>
    ${cardPromotion(c)}
    ${running}
    <dl class="infra-facts">
      ${fact("Running", current)}
      ${fact("SLO", sloText(c.slo), c.slo ? null : "none")}
      ${fact("Uptime 24h", fmtPct(c.uptime_24h), uptimeLevel(c.uptime_24h, c.slo))}
      ${fact("Uptime 7d", fmtPct(c.uptime_7d), uptimeLevel(c.uptime_7d, c.slo))}
      ${fact(`Uptime, SLO window (${c.slo ? c.slo.window_days : 28}d)`, fmtPct(c.uptime_window), uptimeLevel(c.uptime_window, c.slo))}
      ${fact("Error budget", budgetText(c.error_budget), budgetLevel(c.error_budget))}
      ${fact("Last check", c.last_check ? fmtAgo(now, c.last_check) : MISSING)}
    </dl>
    ${checks(c, now)}
    ${incidents(c, now)}
    <dl class="infra-facts sys-dora" aria-label="DORA keys">
      ${doraRows(c.dora).map(r => fact(r.label, r.value)).join("")}
    </dl>
  </article>`;
}

function deployments(report) {
  const rows = deploymentRows(report);
  if (!rows.length) {
    return `<section class="bk-section"><h3>Deployments</h3><div class="empty">No deployment recorded yet.
      <code>factory deploy start</code> and <code>factory deploy finish</code> record one.</div></section>`;
  }
  const body = rows.map(r => `<tr class="${r.standsOut ? "sys-failed" : ""}">
      <td class="bk-nowrap">${esc(r.environment)}</td>
      <td><div class="mono">${esc(r.release)}</div>${r.previous ? `<div class="sub mono">was ${esc(r.previous)}</div>` : ""}</td>
      <td><span class="tag sys-badge" data-tone="${r.tone}">${esc(r.status)}</span></td>
      <td class="bk-nowrap">${esc(r.duration)}</td>
      <td>${esc(r.who)}</td>
      <td class="bk-nowrap">${esc(r.when)}</td>
      <td class="bk-${r.verification.level}">${esc(r.verification.text)}${r.reason ? `<div class="sub">${esc(r.reason)}</div>` : ""}</td>
    </tr>`).join("");
  return `<section class="bk-section"><h3>Deployments <span class="sub">running first, then newest</span></h3>
    <div class="bk-scroll"><table>
      <thead><tr><th>Environment</th><th>Release</th><th>Status</th><th>Duration</th><th>Who</th><th>Started</th><th>Verification</th></tr></thead>
      <tbody>${body}</tbody>
    </table></div></section>`;
}

function releases(report) {
  const rows = releaseRows(report);
  if (!rows.length) return "";
  const body = rows.map((r, index) => `<tr>
      <td class="mono">${esc(r.commit)}${r.dirty ? ` <span class="tag warn">dirty</span>` : ""}</td>
      <td>${r.name ? esc(r.name) : `<span class="infra-missing">${MISSING}</span>`}</td>
      <td>${esc(r.scope)}</td>
      <td class="bk-nowrap">${esc(r.firstSeen)}</td>
      <td>${r.runningOn.length ? r.runningOn.map(e => `<span class="tag sys-badge" data-tone="ok">${esc(e)}</span>`).join(" ") : `<span class="sub">not running anywhere</span>`}</td>
      <td class="bk-nowrap">${r.deployments}${r.failed ? ` <span class="bk-bad">(${r.failed} failed)</span>` : ""}</td>
      <td>${promotionChoices(report, report.releases[index]).map(promotionButton).join(" ")}</td>
    </tr>`).join("");
  return `<section class="bk-section"><h3>Releases <span class="sub">newest first</span></h3>
    <div class="bk-scroll"><table>
      <thead><tr><th>Commit</th><th>Version</th><th>Scope</th><th>First seen</th><th>Running on</th><th>Deployments</th><th>Promote</th></tr></thead>
      <tbody>${body}</tbody>
    </table></div></section>`;
}

function empty() {
  return `<article class="infra-card infra-empty">
    <header class="infra-card-head"><h3>No environments declared</h3></header>
    <p class="infra-hint">An environment is where a scope's own system runs -- staging, production, a review
      environment. Declare them in the scope's <code>.factory/config.yaml</code> and restart the daemon; Factory then
      runs their health checks, and every deployment recorded against them shows up here:</p>
    <pre class="infra-snippet"><code>${esc(CONFIG_SNIPPET)}</code></pre>
  </article>`;
}

// ------------------------------------------------------------------ the view

export function renderEnvironments() {
  const unavailable = $("environments-unavailable");
  if (unavailable) unavailable.hidden = !state.environmentsUnavailable;
  const failed = $("environments-error");
  if (failed) {
    failed.textContent = state.environmentsError || "";
    failed.hidden = !state.environmentsError;
  }
  const stamp = $("environments-generated");
  const report = state.environments;
  if (stamp) stamp.textContent = report ? `as of ${new Date(report.generated_at).toISOString().slice(11, 19)} UTC` : "";
  const page = $("environments");
  if (!page) return;
  if (!report) {
    page.innerHTML = "";
    page.hidden = true;
    return;
  }
  page.hidden = false;
  const now = report.generated_at;
  const cards = report.environments.length
    ? `<div class="sys-cards">${report.environments.map(c => card(c, now)).join("")}</div>`
    : empty();
  page.innerHTML = [promotionNoticeHTML(), cards, deployments(report), releases(report)].join("");
}

export function wireEnvironments() {
  const refresh = $("environments-refresh");
  if (refresh) refresh.onclick = () => { void refreshEnvironments(); };
  const page = $("environments");
  if (page) page.onclick = event => {
    const button = event.target.closest?.("[data-environment-promote]");
    if (button && !button.disabled) void promoteEnvironment(button.dataset.environmentPromote, button.dataset.deployment);
  };
}
