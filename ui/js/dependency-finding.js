//! The detailed vulnerability card shared by L2 Dependencies and L1 Doctor.
//! Factory's running findings live only in Doctor, so both views must render
//! the same evidence rather than maintaining a lossy summary for one of them.

import { esc, statusBadge } from "./core.js";
import {
  affectedLabel,
  exploitSignals,
  pathLabel,
  scanLabel,
} from "./dependencies-model.js";

function ratingLabel(rating) {
  return [rating.source, rating.method, rating.score, rating.vector]
    .filter((value) => value !== null && value !== undefined && value !== "")
    .join(" · ");
}

/// A finding's authored VEX, escaped: state, justification and response, or
/// `none` when nobody has assessed it.
function vexLabel(finding) {
  if (!finding.vex_state) return "none";
  const response = (finding.vex_response || []).join(", ");
  return [finding.vex_state, finding.vex_justification, response]
    .filter(Boolean)
    .map(esc)
    .join(" · ");
}

export function findingCard(finding) {
  const signals = exploitSignals(finding);
  const ratings = (finding.ratings || []).map((rating) =>
    `<li>${[rating.severity, ratingLabel(rating)].filter(Boolean).map(esc).join(" · ")}</li>`
  ).join("");
  const vex = vexLabel(finding);
  return `<details class="dep-finding">
    <summary>
      ${statusBadge(finding.status)}
      <span class="dep-severity sev-${esc(finding.severity)}">${esc(finding.severity)}</span>
      <strong>${esc(finding.id)}</strong>
      <span>${esc(affectedLabel(finding.affected))}</span>
      <span class="sub">${esc(finding.state)}</span>
    </summary>
    <dl>
      <dt>Ratings</dt><dd>${ratings ? `<ul>${ratings}</ul>` : "—"}</dd>
      <dt>Affected</dt><dd>${esc(affectedLabel(finding.affected))}<br><code>${esc(finding.affected.bom_ref)}</code></dd>
      <dt>Dependency path</dt><dd>${esc(pathLabel(finding.affected))}</dd>
      <dt>Scan</dt><dd>${esc(scanLabel(finding.scan))}<br>run <code>${esc(finding.scan.attachment.run_id)}</code></dd>
      <dt>Fixed version</dt><dd>${esc(finding.fixed_version || "—")}</dd>
      <dt>VEX</dt><dd>${vex}</dd>
      <dt>Exploit signals</dt><dd>${signals.length ? signals.map((signal) => `<span class="tag warn">${esc(signal)}</span>`).join(" ") : "none"}</dd>
    </dl>
  </details>`;
}
