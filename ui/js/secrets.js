//! L2 Environment, tab 2: the correction that Factory injects no credentials,
//! and the honest inventory of what an agent can already reach because it
//! runs as the daemon's owner. Reads the same `/api/environment` answer
//! `sandboxes.js` fetches -- see `loadEnvironment` there. Never a value, here
//! or anywhere below: presence only, on every row.

import { $, esc, state } from "./core.js";
import { inScope } from "./scopes.js";

const CORRECTION = "Factory injects no credentials into a session -- the only environment it adds is six " +
  "FACTORY_* variables (scope, socket, binary, token, and a run's task id and attempt). That is not the same " +
  "as the agent having none: it runs as the owner, in the owner's home, and can already reach anything below.";

const BOUNDARY = "Values are never shown here, never read by Factory, and never will be -- every row below is a " +
  "path checked for existence, nothing more.";

// A fact about how the daemon runs its agents, not about the answer to any
// one request -- so it lives here rather than in the payload, and is still on
// screen when the fetch fails. That is the moment it matters most: a page that
// cannot list what is reachable must not also stop saying that everything is.
const REACHABILITY = "Every agent runs as the daemon's owner, in the owner's home, so every credential below " +
  "that is present is already reachable by every agent on this machine -- Factory injects none of them, and " +
  "nothing here narrows what an agent can reach.";

/// Narrow the inventory to the rail's inclusive scope selection, the same way
/// `sandboxes.js` narrows its own rows.
///
/// A row with no scope is a credential in the owner's home. It belongs to no
/// scope and is reachable from every one of them, so it survives every
/// selection -- hiding it under a scope filter would have the page quietly
/// unsay the one thing this tab exists to say.
export function visibleCredentials(rows, contains = inScope) {
  return (rows || []).filter(row => !row.scope || contains(row.scope));
}

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
  if (reach) reach.textContent = REACHABILITY;
  const failed = $("secrets-error");
  if (failed) {
    failed.textContent = state.environmentError || "";
    failed.hidden = !state.environmentError;
  }

  const body = $("secrets");
  if (!body) return;
  const rows = state.environmentError
    ? []
    : visibleCredentials(state.environment && state.environment.credentials);
  body.innerHTML = rows.map(credentialRow).join("");
  const empty = $("noSecrets");
  if (empty) empty.hidden = rows.length !== 0 || !!state.environmentError;
}
