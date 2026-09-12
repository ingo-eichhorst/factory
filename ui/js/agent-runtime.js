//! The connection behind the agents: one adapter-neutral card per effective
//! runtime, with concrete details supplied by the adapter that owns them.

import { $, api, esc, since, state } from "./core.js";
import { inScope } from "./scopes.js";

const VIEWS = new Set(["occupancy", "roster", "agent-runtime"]);

export function agentViewFromTail(value) {
  return VIEWS.has(value) ? value : "occupancy";
}

export function agentViewTail(view) {
  return view === "occupancy" ? [] : [agentViewFromTail(view)];
}

/// Keep one grouped connection card, but narrow the scope names written on it
/// to the rail's inclusive selection. `contains` is passed in so this remains
/// a small, testable shape rather than a second implementation of scope paths.
export function visibleRuntimeConnections(runtimes, contains = inScope) {
  return (runtimes || []).map(runtime => ({
    ...runtime,
    scopes: (runtime.scopes || []).filter(contains),
  })).filter(runtime => runtime.scopes.length > 0);
}

const STATE_NOTE = {
  healthy: "Connected and compatible.",
  stopped: "The runtime server is stopped. Start it outside Factory, then refresh.",
  unreachable: "Factory could not reach the configured runtime client or server.",
  incompatible: "The client and server protocols are incompatible. Install matching versions.",
  degraded: "Connected, but the runtime reports that its server needs a restart.",
  unsupported: "This runtime adapter does not report connection diagnostics.",
  error: "Factory could not obtain a valid connection diagnostic.",
};

function peer(label, value) {
  if (!value) return "";
  return `<div><dt>${label}</dt><dd><code>${esc(value.version)}</code><span class="sub">protocol ${esc(value.protocol)}</span></dd></div>`;
}

function answer(value) {
  if (value === true) return "yes";
  if (value === false) return "no";
  return "not reported";
}

export function runtimeConnectionCard(runtime) {
  const stateName = runtime.state || "error";
  const capabilities = (runtime.capabilities || []).length
    ? runtime.capabilities.map(capability => `<span class="tag">${esc(capability)}</span>`).join("")
    : `<span class="sub">none reported</span>`;
  const endpoint = runtime.endpoint
    ? `<div><dt>Endpoint</dt><dd><code>${esc(runtime.endpoint)}</code></dd></div>`
    : "";
  const session = runtime.session
    ? `<div><dt>Session</dt><dd><code>${esc(runtime.session)}</code></dd></div>`
    : "";
  const error = runtime.error ? `<div class="err">${esc(runtime.error)}</div>` : "";

  return `<article class="runtime-card" data-state="${esc(stateName)}">
    <div class="runtime-head">
      <div><h3>${esc(runtime.runtime)}</h3><div class="sub">${esc(runtime.description)}</div></div>
      <span class="tag ${runtime.source !== "builtin" && runtime.source !== "missing" ? "plug" : ""}">${esc(runtime.source)}</span>
      <span class="runtime-state rt-${esc(stateName)}">${esc(stateName)}</span>
    </div>
    <p class="runtime-note">${esc(STATE_NOTE[stateName] || STATE_NOTE.error)}</p>
    ${error}
    <dl class="runtime-facts">
      ${session}${endpoint}
      ${peer("Client", runtime.client)}${peer("Server", runtime.server)}
      <div><dt>Compatible</dt><dd>${answer(runtime.compatible)}</dd></div>
      <div><dt>Restart needed</dt><dd>${answer(runtime.restart_needed)}</dd></div>
    </dl>
    <div class="runtime-line"><span class="sub">Capabilities</span><span class="runtime-tags">${capabilities}</span></div>
    <div class="runtime-line"><span class="sub">Scopes</span><span>${runtime.scopes.map(esc).join(" · ")}</span></div>
    <div class="runtime-checked sub">checked ${since(runtime.checked_at)} ago</div>
  </article>`;
}

export function renderRuntimeConnections() {
  if (state.runtimeConnectionError) {
    $("agent-runtime").innerHTML = `<div class="err runtime-load-error">${esc(state.runtimeConnectionError)}</div>`;
    return;
  }
  const runtimes = visibleRuntimeConnections(state.runtimeConnections);
  $("agent-runtime").innerHTML = runtimes.length
    ? runtimes.map(runtimeConnectionCard).join("")
    : `<div class="empty">No runtime connection is used by this scope.</div>`;
}

export async function loadRuntimeConnections() {
  const button = $("runtime-refresh");
  if (button) button.disabled = true;
  try {
    const answer = await api("/api/agent-runtime");
    state.runtimeConnections = answer.runtimes || [];
    state.runtimeConnectionError = null;
    renderRuntimeConnections();
  } catch (error) {
    // Do not let a scope re-render turn a failed refresh back into the last
    // green answer. The next successful probe replaces both facts together.
    state.runtimeConnectionError = error.message;
    renderRuntimeConnections();
  } finally {
    if (button) button.disabled = false;
  }
}
