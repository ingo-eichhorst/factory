//! L2 Environment, tab 1: where a declared agent's runs execute. `secrets.js`
//! is the other tab over the same `/api/environment` answer; `loadEnvironment`
//! lives here because the sandbox rows are this endpoint's primary content,
//! but it only fetches and sets state -- app.js renders both tabs from it, the
//! same way it is the one file that already knows every view there is.

import { $, api, esc, state } from "./core.js";
import { inScope } from "./scopes.js";

/// Narrow the sandbox rows to the rail's inclusive scope selection, the same
/// way `agent-runtime.js` narrows its own scope list.
export function visibleSandboxes(rows, contains = inScope) {
  return (rows || []).filter(row => contains(row.scope));
}

function sandboxRow(row) {
  const declared = row.sandbox !== "none";
  return `<tr>
    <td><div class="title">${esc(row.scope)}</div><div class="sub">${esc(row.scope_path)}</div></td>
    <td>${esc(row.runtime)}</td>
    <td>${esc(row.agent)}</td>
    <td>${esc(row.harness)}</td>
    <td>${esc(row.lifetime)}</td>
    <td>${esc(row.sandbox)}${declared ? ` <span class="tag warn">declared, not enforced</span>` : ""}</td>
    <td>${row.worktree_capable ? "yes" : "no"}</td>
  </tr>`;
}

export function renderSandboxes() {
  const note = $("sandboxes-note");
  if (note) {
    // True whether or not the fetch worked, so an error goes in an element of
    // its own beside it rather than over the top of it.
    note.textContent =
      "Today every row is this Mac, this folder, no limits. A sandbox value below says what an agent's declaration " +
      "claims -- none of it is enforced yet, so “docker” and “srt” change nothing about how the agent actually starts.";
  }
  const failed = $("sandboxes-error");
  if (failed) {
    failed.textContent = state.environmentError || "";
    failed.hidden = !state.environmentError;
  }
  const body = $("sandboxes");
  if (!body) return;
  const rows = state.environmentError ? [] : visibleSandboxes(state.environment && state.environment.sandboxes);
  body.innerHTML = rows.map(sandboxRow).join("");
  const empty = $("noSandboxes");
  if (empty) empty.hidden = rows.length !== 0 || !!state.environmentError;
}

/// The one fetch behind both tabs. Sets `state.environment` (or
/// `state.environmentError`, never both) and nothing else -- app.js renders
/// both views from whichever it finds, in `refreshEnvironment`.
export async function loadEnvironment() {
  const buttons = [$("environment-refresh"), $("secrets-refresh")].filter(Boolean);
  for (const b of buttons) b.disabled = true;
  try {
    state.environment = await api("/api/environment");
    state.environmentError = null;
  } catch (error) {
    state.environment = null;
    state.environmentError = error.message;
  } finally {
    for (const b of buttons) b.disabled = false;
  }
}
