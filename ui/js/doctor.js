//! L1 Doctor: the installed Factory binaries against the newest scanned
//! Factory build. The daemon owns the current/behind/missing verdict.

import { $, api, esc, state } from "./core.js";
import { scanLabel, shapeFindings } from "./dependencies-model.js";
import { findingCard } from "./dependency-finding.js";
import {
  doctorFailure,
  findingSummary,
  gatewaySummary,
  identityText,
  scanAge,
  statusSummary,
} from "./doctor-model.js";

export async function loadDoctor() {
  const button = $("doctor-refresh");
  if (button) button.disabled = true;
  try {
    state.doctor = (await api("/api/doctor")).report;
    state.doctorError = null;
    state.doctorUnavailable = false;
  } catch (error) {
    state.doctor = null;
    state.doctorUnavailable = doctorFailure(error) === "unavailable";
    state.doctorError = state.doctorUnavailable ? null : error.message;
  } finally {
    if (button) button.disabled = false;
  }
}

function evidenceCard(title, document, now) {
  if (!document) return `<article class="infra-card doctor-evidence missing">
    <header class="infra-card-head"><h3>${esc(title)}</h3><span class="tag">missing</span></header>
    <p class="infra-hint">No ${esc(title.toLowerCase())} SBOM has been attached for the factory scope.</p>
  </article>`;
  const scan = document.sbom;
  return `<article class="infra-card doctor-evidence">
    <header class="infra-card-head"><h3>${esc(title)}</h3><span class="tag">${esc(document.state)}</span></header>
    <div class="doctor-identity">${esc(identityText(document))}</div>
    <div class="sub">${esc(scanAge(now, document))} · ${esc(scanLabel(scan))}</div>
    <div class="sub">run <code>${esc(scan.attachment.run_id)}</code> · CycloneDX ${esc(scan.attachment.spec_version)}</div>
  </article>`;
}

/// The OpenShell gateways sandboxed agents use (#234). Absent when no agent
/// declares `sandbox: openshell`.
function gatewaySection(rows) {
  if (!rows || !rows.length) return "";
  return `<section class="doctor-evidence-grid">${rows.map((row) => {
    const g = gatewaySummary(row);
    return `<article class="infra-card doctor-evidence">
      <header class="infra-card-head"><h3>OpenShell gateway ${esc(g.name)}</h3><span class="tag bk-badge" data-level="${esc(g.level)}">${esc(g.label)}</span></header>
      ${g.facts ? `<div class="sub">${esc(g.facts)}</div>` : ""}
      ${g.started ? `<div class="sub">${esc(g.started)}</div>` : ""}
      ${g.detail ? `<p class="infra-hint">${esc(g.detail)}</p>` : ""}
    </article>`;
  }).join("")}</section>`;
}

export function renderDoctor() {
  const unavailable = $("doctor-unavailable");
  if (unavailable) unavailable.hidden = !state.doctorUnavailable;
  const error = $("doctor-error");
  if (error) {
    error.textContent = state.doctorError || "";
    error.hidden = !state.doctorError;
  }
  const page = $("doctor");
  if (!page) return;
  const report = state.doctor;
  page.hidden = !report;
  if (!report) {
    page.innerHTML = "";
    return;
  }
  const status = statusSummary(report.status);
  const findings = shapeFindings(report.findings);
  const counts = findingSummary(findings);
  page.innerHTML = `<section class="infra-card doctor-hero" data-level="${esc(status.level)}">
      <header class="infra-card-head"><h3>${esc(status.label)}</h3><span class="tag">Factory dependencies</span></header>
      <p class="infra-hint">${esc(status.detail)}</p>
    </section>
    <section class="doctor-evidence-grid">
      ${evidenceCard("Newest build", report.built, report.now)}
      ${evidenceCard("Installed binaries", report.running, report.now)}
    </section>
    ${gatewaySection(report.openshell)}
    <section class="doctor-findings">
      <h3>Running findings <span class="sub">${counts.open} open · ${counts.assessed} assessed · ${counts.total} total</span></h3>
      ${findings.length ? `<div class="doctor-finding-list">${findings.map(findingCard).join("")}</div>`
        : `<div class="empty">No findings in the newest running scan.</div>`}
    </section>`;
}

export async function refreshDoctor() {
  await loadDoctor();
  renderDoctor();
}

export function wireDoctor() {
  $("doctor-refresh").onclick = refreshDoctor;
}
