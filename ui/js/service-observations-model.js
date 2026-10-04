//! Evidence shaping only. Never turn absence, allow/deny, or an endpoint
//! match into a compliance verdict or a claim of complete service usage.
import { esc } from "./core.js";

export function observedAccesses(evidence) {
  const unique = new Map();
  for (const capture of evidence?.captures || []) {
    for (const access of capture.accesses || []) {
      const key = JSON.stringify([capture.run_id, access.at, access.transport,
        access.target, access.disposition, access.process, access.policy]);
      if (!unique.has(key)) unique.set(key, { ...access, capture });
    }
  }
  return [...unique.values()].sort((a, b) => String(b.at).localeCompare(String(a.at)));
}

function authority(raw) {
  if (typeof raw !== "string") return null;
  try {
    const value = new URL(raw.includes("://") ? raw : `https://${raw}`);
    if (value.username || value.password || !["http:", "https:"].includes(value.protocol)) return null;
    return `${value.hostname.toLowerCase()}:${value.port || (value.protocol === "https:" ? 443 : 80)}`;
  } catch { return null; }
}

export function matchingAccesses(service, evidence) {
  const endpoints = (service.endpoints || []).map(authority).filter(Boolean);
  return observedAccesses(evidence).filter((access) => {
    if (access.transport !== service.transport) return false;
    if (service.agents?.length && !service.agents.includes(access.capture.agent)) return false;
    if (service.transport === "network") return endpoints.includes(authority(access.target));
    return service.path === access.target;
  });
}

export function observedCell(service, evidence) {
  const rows = matchingAccesses(service, evidence);
  if (!rows.length) return '<span class="sub">unknown · no matching access evidence</span>';
  const counts = new Map();
  for (const row of rows) counts.set(row.disposition, (counts.get(row.disposition) || 0) + 1);
  return [...counts].map(([disposition, count]) => `${esc(count)} ${esc(disposition)}`).join(" · ")
    + '<div class="sub">partial log · same endpoint, not a verdict</div>';
}

export function evidenceCards(evidence) {
  const rows = observedAccesses(evidence);
  const notices = (evidence?.findings || []).map((finding) => `<p class="env-note">${esc(finding)}</p>`).join("");
  const captures = (evidence?.captures || []).map((capture) => `<details class="dep-doc">
    <summary>run <code>${esc(capture.run_id)}</code> · ${esc(capture.agent)} · partial enforcement log</summary>
    <div class="sub">${esc(capture.source)} · ${esc(capture.sandbox)} · captured ${esc(capture.captured_at)}</div>
    <div class="sub">task <code>${esc(capture.task_id)}</code> · evidence <code>${esc(capture.id)}</code></div>
    ${capture.issue ? `<p class="env-note">${esc(capture.issue)}</p>` : ""}
  </details>`).join("");
  const accesses = rows.map((row) => `<article class="dep-doc">
    <strong>${esc(row.transport)} <code>${esc(row.target)}</code></strong> · ${esc(row.disposition)}
    <div class="sub">${esc(row.at)} · ${esc(row.capture.agent)} · run <code>${esc(row.capture.run_id)}</code></div>
    <div class="sub">${esc(row.process || "process unknown")} · policy ${esc(row.policy || "unknown")}</div>
  </article>`).join("");
  const empty = rows.length ? "" : '<p class="empty">Access is unknown: no supported access records in the available evidence.</p>';
  return '<p class="env-note">Observed requests, not a compliance verdict. Evidence is a bounded, partial log; no record does not mean no access. Socket and file use need actual access records; configured paths and listeners are not observations.</p>'
    + notices + empty + accesses + captures;
}
