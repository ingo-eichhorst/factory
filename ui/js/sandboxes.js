//! L2 Environment, tab 1: where a declared agent's runs execute. `secrets.js`
//! is the other tab over the same `/api/environment` answer; `loadEnvironment`
//! lives here because the sandbox rows are this endpoint's primary content,
//! but it only fetches and sets state -- app.js renders both tabs from it, the
//! same way it is the one file that already knows every view there is.

import { $, api, esc, state } from "./core.js";
import { inScope } from "./scopes.js";
import { dateBadges } from "./dates.js";

/// Narrow the sandbox rows to the rail's inclusive scope selection, the same
/// way `agent-runtime.js` narrows its own scope list.
export function visibleSandboxes(rows, contains = inScope) {
  return (rows || []).filter(row => contains(row.scope));
}

/// The tag beside a row's sandbox value: what dispatch actually does with
/// it. `openshell` is enforced -- the run starts inside the sandbox or not at
/// all (#218); `docker` and `srt` are still only what a declaration says.
export function sandboxTag(row) {
  if (!row?.sandbox || row.sandbox === "none") return "";
  return row.enforced
    ? ` <span class="tag ok">enforced</span>`
    : ` <span class="tag warn">declared, not enforced</span>`;
}

/// Whether a `sandbox: openshell` agent's prerequisites are in place ahead
/// of any run (#234): `ready`, `preparing`, or `needs` the one thing named.
/// An openshell row the daemon has not judged yet says so; any other row
/// has nothing to say.
const READINESS_TONE = { ready: "ok", needs: "bad", preparing: "warn" };

export function readinessTag(row) {
  if (row?.sandbox !== "openshell") return "";
  const r = row.readiness;
  if (!r) return ` <span class="tag">checking</span>`;
  const tone = READINESS_TONE[r.state] || "warn";
  const title = r.thing ? ' title="' + esc(r.thing) + '"' : "";
  return ` <span class="tag sys-badge" data-tone="${tone}"${title}>${esc(r.state)}</span>`;
}

/// The line under a row that is not ready: what it needs and the command
/// that supplies it, or what the daemon is doing about it. Notes ride along
/// for a ready row -- an image rebuilding, an expiry ahead.
export function readinessDetail(readiness) {
  if (!readiness) return "";
  const lines = [];
  if (readiness.state === "needs" && readiness.thing) lines.push(`needs ${esc(readiness.thing)}`);
  if (readiness.state === "preparing" && readiness.thing) lines.push(esc(readiness.thing));
  if (readiness.command) lines.push(`<code>${esc(readiness.command)}</code>`);
  for (const note of readiness.notes || []) lines.push(esc(note));
  for (const e of readiness.expiring || []) {
    const when = e.days_left < 0
      ? "expired on " + esc(e.expires)
      : "expires on " + esc(e.expires) + " (" + esc(String(e.days_left)) + " days)";
    lines.push(esc(e.provider) + "'s credential " + when);
  }
  return lines.map(line => `<div class="sub">${line}</div>`).join("");
}

function sandboxRow(row) {
  return `<tr>
    <td><div class="title">${esc(row.scope)}</div><div class="sub">${esc(row.scope_path)}</div></td>
    <td>${esc(row.runtime)}</td>
    <td>${esc(row.agent)} ${dateBadges({ scope: row.scope, agent: row.agent })}</td>
    <td>${esc(row.harness)}</td>
    <td>${esc(row.lifetime)}</td>
    <td>${esc(row.sandbox)}${sandboxTag(row)}${readinessTag(row)}${readinessDetail(row.readiness)}</td>
    <td>${row.worktree_capable ? "yes" : "no"}</td>
  </tr>`;
}

export function renderSandboxes() {
  const note = $("sandboxes-note");
  if (note) {
    // True whether or not the fetch worked, so an error goes in an element of
    // its own beside it rather than over the top of it.
    note.textContent =
      "A row with no sandbox runs on this Mac, in this folder, with no limits. “openshell” is enforced: each task run " +
      "starts inside its own NVIDIA OpenShell sandbox under the policy the scope's config declares, and fails rather than " +
      "start on the host. “docker” and “srt” are still declared only -- they change nothing about how the agent starts.";
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
