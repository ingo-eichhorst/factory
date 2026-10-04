//! L2 Environment, Dependencies: one selected scope's newest SBOM per
//! lifecycle, derived findings, and declared services.

import { $, api, esc, state } from "./core.js";
import { writeHash } from "./scopes.js";
import { findingCard } from "./dependency-finding.js";
import { observedCell, evidenceCards } from "./service-observations-model.js";
import {
  credentialState,
  findingCounts,
  scanLabel,
  serviceTarget,
  shapeFindings,
  visibleDocuments,
  visibleFindings,
} from "./dependencies-model.js";

function renderChrome() {
  const segment = state.dependenciesSegment || "sbom";
  const bar = $("dependencies-seg");
  if (bar) for (const button of bar.querySelectorAll("button")) {
    button.classList.toggle("on", button.dataset.seg === segment);
  }
  $("dependencies-pane-sbom").hidden = segment !== "sbom";
  $("dependencies-pane-services").hidden = segment !== "services";
}

function documentCard(row) {
  const scan = row.sbom;
  const vulnerability = row.vulnerabilities;
  return `<article class="dep-doc">
    <div><span class="tag">${esc(row.state)}</span> <strong>${esc(scan.attachment.filename)}</strong></div>
    <div class="sub">${esc(scanLabel(scan))}</div>
    <div class="sub">run <code>${esc(scan.attachment.run_id)}</code> · CycloneDX ${esc(scan.attachment.spec_version)}</div>
    <div>${vulnerability
      ? `findings <code>${esc(vulnerability.attachment.filename)}</code>`
      : `<span class="sub">no vulnerability document attached by this scan</span>`}</div>
  </article>`;
}

function credentialCell(name, credential) {
  if (!name) return "—";
  const tone = credential === "present" ? "warn" : "";
  return `<code>${esc(name)}</code> <span class="tag ${tone}">${esc(credential)}</span>`;
}

function serviceRow(row, evidence) {
  const credential = credentialState(row);
  const scope = (row.agents || []).length ? row.agents.join(", ") : "every declared agent";
  const access = [row.effects, row.direction].filter(Boolean).join(" / ") || "—";
  return `<tr>
    <td><strong>${esc(row.name)}</strong><div class="sub">${esc(scope)}</div></td>
    <td>${esc(row.transport)}</td>
    <td><code>${esc(serviceTarget(row))}</code></td>
    <td>${esc(access)}</td>
    <td>${(row.data || []).map((d) => `<span class="tag">${esc(d)}</span>`).join(" ") || "—"}</td>
    <td>${credentialCell(row.credential, credential)}</td>
    <td>${observedCell(row, evidence)}</td>
  </tr>`;
}

export function renderDependencies() {
  renderChrome();
  const note = $("dependencies-note");
  const error = $("dependencies-error");
  const selected = state.scope;
  if (!selected) {
    note.textContent = "Dependencies are deliberately scoped to one product. Select one scope in the rail.";
    error.hidden = true;
    $("dependencies-documents").innerHTML = "";
    $("dependencies-findings").innerHTML = "";
    $("dependencies-services").innerHTML = "";
    $("dependencies-observations").innerHTML = "";
    $("no-dependency-documents").hidden = false;
    $("no-dependency-findings").hidden = true;
    $("no-dependency-services").hidden = true;
    return;
  }
  note.textContent = `Scope ${selected}. Status is derived from immutable scan attachments and authored VEX; it cannot be set here.`;
  error.textContent = state.dependenciesError || "";
  error.hidden = !state.dependenciesError;
  const report = state.dependenciesError ? null : state.dependencies;

  const documents = visibleDocuments(report?.documents, selected);
  $("dependencies-documents").innerHTML = documents.map(documentCard).join("");
  $("no-dependency-documents").hidden = documents.length !== 0 || !!state.dependenciesError;

  const findings = shapeFindings(visibleFindings(report?.findings, selected));
  const counts = findingCounts(findings);
  $("dependencies-counts").textContent = `${counts.open} open · ${counts.assessed} assessed · ${counts.resolved} resolved · ${counts.stale} stale`;
  $("dependencies-findings").innerHTML = findings.map(findingCard).join("");
  $("no-dependency-findings").hidden = findings.length !== 0 || !!state.dependenciesError;

  const services = report?.services || [];
  $("dependencies-services").innerHTML = services.map((service) => serviceRow(service, report?.service_evidence)).join("");
  $("dependencies-observations").innerHTML = evidenceCards(report?.service_evidence);
  $("no-dependency-services").hidden = services.length !== 0 || !!state.dependenciesError;
}

export async function loadDependencies() {
  if (!state.scope) {
    state.dependencies = null;
    state.dependenciesError = null;
    renderDependencies();
    return;
  }
  const refresh = $("dependencies-refresh");
  if (refresh) refresh.disabled = true;
  try {
    const answer = await api(`/api/dependencies?scope=${encodeURIComponent(state.scope)}`);
    state.dependencies = answer.report;
    state.dependenciesError = null;
  } catch (error) {
    state.dependencies = null;
    state.dependenciesError = error.message;
  } finally {
    if (refresh) refresh.disabled = false;
  }
  renderDependencies();
}

export function wireDependencies() {
  $("dependencies-refresh").onclick = loadDependencies;
  for (const button of $("dependencies-seg").querySelectorAll("button")) {
    button.onclick = () => {
      state.dependenciesSegment = button.dataset.seg;
      renderDependencies();
      writeHash();
    };
  }
}
