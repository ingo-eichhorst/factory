//! L1 Mac (#260): the host's power mode -- Automatic, High performance or
//! Energy saving -- as a three-way segmented control, read from and set
//! through `/api/host/power-mode`.
//!
//! The click is the confirmation: there is no modal. A segment is only live
//! when the daemon said it may set that mode; until the sudoers rule is
//! installed the control is drawn disabled and the rule, with the command
//! that installs it, is shown instead -- the one step that stays a person's.
//!
//! Every decision lives in `mac-model.js`, which the Node tests import; this
//! file only puts its answers on screen. Not polled: every read asks `sudo
//! -n -l`, which a host may log, so it is fetched on show and on Refresh.

import { $, api, esc, state } from "./core.js";
import { fmtWhen } from "./backup-model.js";
import {
  changeRows,
  headline,
  macFailure,
  installStep,
  macState,
  modeLabel,
  needsRule,
  reading,
  segments,
  setBody,
  sourceRows,
  stateText,
} from "./mac-model.js";

// ------------------------------------------------------------------ fetching

/// Sets `state.mac`, or `state.macError` with `state.macUnavailable` saying
/// whether the failure was only a daemon too old to serve the endpoint.
export async function loadMac() {
  try {
    state.mac = (await api("/api/host/power-mode")).report;
    state.macError = null;
    state.macUnavailable = false;
  } catch (error) {
    state.mac = null;
    state.macUnavailable = macFailure(error) === "unavailable";
    state.macError = state.macUnavailable ? null : error.message;
  }
}

export async function refreshMac() {
  const button = $("mac-refresh");
  if (button) button.disabled = true;
  try {
    await loadMac();
    renderMac();
  } finally {
    if (button) button.disabled = false;
  }
}

/// One click: set it, then draw the report the daemon answers with. A
/// refusal keeps the last report on screen and says why.
async function choose(mode) {
  if (state.macBusy) return;
  state.macBusy = mode;
  state.macError = null;
  renderMac();
  try {
    state.mac = (await api("/api/host/power-mode", { method: "POST", body: setBody(mode) })).report;
  } catch (error) {
    state.macError = `Could not set ${modeLabel(mode)}: ${error.message}`;
  } finally {
    state.macBusy = null;
    renderMac();
  }
}

// ------------------------------------------------------------------ pieces

function fact(label, value) {
  return `<div class="infra-fact"><dt>${esc(label)}</dt><dd>${esc(value)}</dd></div>`;
}

function control(report) {
  const segs = segments(report, { busy: state.macBusy });
  if (!segs) return "";
  return `<fieldset class="seg mac-seg" aria-label="Power mode">${segs.map(s => `
    <button type="button" data-mode="${esc(s.mode)}" class="${s.on ? "on" : ""}" aria-pressed="${s.on}"
      ${s.disabled ? "disabled" : ""} title="${esc(s.title)}">${esc(s.label)}</button>`).join("")}
  </fieldset>`;
}

function hero(report) {
  const current = macState(report);
  const { mixed } = reading(report);
  const rows = current === "unsupported" ? "" : `<dl class="infra-facts">
      ${sourceRows(report).map(r => fact(r.source, r.label)).join("")}
      ${fact("Offered", (report.supported || []).map(modeLabel).join(", ") || "--")}
    </dl>`;
  return `<article class="infra-card mac-hero" data-state="${esc(current)}" aria-labelledby="mac-hero-title">
    <header class="infra-card-head">
      <h3 id="mac-hero-title">Power mode</h3>
      <span class="tag mac-badge" data-state="${esc(current)}">${esc(current === "read-only" ? "read-only" : current === "editable" ? "can change" : "not supported")}</span>
      ${mixed ? `<span class="tag warn">mixed</span>` : ""}
    </header>
    <div class="bk-headline"><div class="bk-age">${esc(headline(report))}</div>
      <div class="sub">${esc(stateText(report))}</div></div>
    ${control(report)}
    ${rows}
    ${(report.notes || []).map(n => `<p class="env-note warn">${esc(n)}</p>`).join("")}
  </article>`;
}

function rule(report) {
  if (!needsRule(report)) return "";
  const s = report.sudoers;
  const step = installStep(s);
  const install = step.su
    ? `<p class="infra-hint">${esc(step.lead)} First switch to that account:</p>
      <pre class="infra-snippet mac-cmd"><code>${esc(step.su)}</code></pre>
      <p class="infra-hint">then, as that administrator, run this. It checks the rule with <code>visudo -cf</code>
        before installing it, root-owned and read-only:</p>`
    : `<p class="infra-hint">This checks the rule with <code>visudo -cf</code> before installing it, root-owned and
        read-only${step.lead ? `. ${esc(step.lead)}` : ":"}</p>`;
  return `<article class="infra-card mac-rule" aria-labelledby="mac-rule-title">
    <header class="infra-card-head">
      <h3 id="mac-rule-title">Let Factory change it</h3>
      <span class="sub">once, by a person, in a terminal</span>
    </header>
    <p class="infra-hint">The daemon runs <code>sudo -n</code>, which never prompts. Install this rule at
      <code>${esc(s.path)}</code> -- it lets <code>${esc(s.user)}</code> run exactly these three commands as root,
      and nothing else:</p>
    <pre class="infra-snippet mac-cmd"><code>${esc(s.rule)}</code></pre>
    ${install}
    <pre class="infra-snippet mac-cmd"><code>${esc(s.install)}</code></pre>
    <p class="infra-hint">Then${step.su ? ", still as the administrator," : ""} <code>${esc(s.check)}</code> checks the
      whole configuration, and Refresh here.</p>
  </article>`;
}

function history(report) {
  const rows = changeRows(report);
  if (!rows.length) return "";
  return `<section class="bk-section"><h3>Changes <span class="sub">newest first</span></h3>
    <div class="bk-scroll"><table><thead><tr><th>When</th><th>Change</th></tr></thead><tbody>${rows.map(r => `
      <tr><td class="bk-nowrap">${esc(fmtWhen(r.at))}</td><td>${esc(r.text)}</td></tr>`).join("")}
    </tbody></table></div></section>`;
}

// ------------------------------------------------------------------ the view

export function renderMac() {
  const unavailable = $("mac-unavailable");
  if (unavailable) unavailable.hidden = !state.macUnavailable;
  const failed = $("mac-error");
  if (failed) {
    failed.textContent = state.macError || "";
    failed.hidden = !state.macError;
  }
  const page = $("mac");
  if (!page) return;
  const report = state.mac;
  if (!report) {
    page.innerHTML = "";
    page.hidden = true;
    return;
  }
  page.hidden = false;
  page.innerHTML = [hero(report), rule(report), history(report)].join("");
  for (const b of page.querySelectorAll("[data-mode]")) {
    b.onclick = () => choose(b.dataset.mode);
  }
}

/// The bar's one button -- wired once, from app.js.
export function wireMac() {
  const refresh = $("mac-refresh");
  if (refresh) refresh.onclick = () => refreshMac();
}
