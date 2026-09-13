//! L2 Environment, tab 2: the correction that Factory injects no credentials,
//! and the honest inventory of what an agent can already reach because it
//! runs as the daemon's owner. Reads the same `/api/environment` answer
//! `sandboxes.js` fetches -- see `loadEnvironment` there. Never a value, here
//! or anywhere below: presence only, on every row.

import { $, esc, state } from "./core.js";

const CORRECTION = "Factory injects no credentials into a session -- the only environment it adds is six " +
  "FACTORY_* variables (scope, socket, binary, token, and a run's task id and attempt). That is not the same " +
  "as the agent having none: it runs as the owner, in the owner's home, and can already reach anything below.";

const BOUNDARY = "Values are never shown here, never read by Factory, and never will be -- every row below is a " +
  "path checked for existence, nothing more.";

function credentialRow(row) {
  return `<tr>
    <td>${esc(row.integration)}</td>
    <td>${esc(row.label)}</td>
    <td><code>${esc(row.path)}</code></td>
    <td>${row.present ? `<span class="tag warn">present</span>` : `<span class="tag">absent</span>`}</td>
  </tr>`;
}

export function renderSecrets() {
  const correction = $("secrets-correction");
  if (correction) correction.textContent = CORRECTION;
  const boundary = $("secrets-boundary");
  if (boundary) boundary.textContent = BOUNDARY;

  const reach = $("secrets-reach");
  if (reach) {
    reach.textContent = state.environmentError ||
      (state.environment && state.environment.reachability_note) || "";
  }

  const body = $("secrets");
  if (!body) return;
  const rows = state.environmentError ? [] : ((state.environment && state.environment.credentials) || []);
  body.innerHTML = rows.map(credentialRow).join("");
}
