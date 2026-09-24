//! L1 Infrastructure: what everything runs on. One answer from
//! `/api/infrastructure`, drawn as a dependency picture read from the bottom
//! up -- the host, the daemon standing on it, and above that the AI accounts
//! the agents' harnesses pay through, with the agents that depend on each.
//!
//! Accounts are declared in the root config and never discovered: the daemon
//! opens no credential file to draw this page, and neither does the page. A
//! key's environment variable is shown by name; its value is never read.
//!
//! Every formatter lives in `infra-model.js`, which the Node tests import; this
//! file only puts their answers on screen.

import { $, api, esc, state } from "./core.js";
import { inScope } from "./scopes.js";
import {
  MISSING,
  PROVIDERS_SNIPPET,
  agentHref,
  daemonUptime,
  diskLevel,
  diskPercent,
  fact,
  fmtBytes,
  fmtInterfaces,
  fmtLoad,
  fmtRuntime,
  fmtUptime,
  groupAgentsByScope,
  infraFailure,
  isEmptyProviders,
  kindBadge,
  visibleAgents,
} from "./infra-model.js";

// ------------------------------------------------------------------ fetching

/// Sets `state.infrastructure`, or `state.infrastructureError` with
/// `state.infrastructureUnavailable` saying whether the failure was only a
/// daemon too old to serve the endpoint. Renders nothing -- app.js does, in
/// `refreshInfrastructure`, the same split `loadEnvironment` keeps.
export async function loadInfrastructure() {
  const button = $("infrastructure-refresh");
  if (button) button.disabled = true;
  try {
    state.infrastructure = await api("/api/infrastructure");
    state.infrastructureError = null;
    state.infrastructureUnavailable = false;
  } catch (error) {
    state.infrastructure = null;
    state.infrastructureUnavailable = infraFailure(error) === "unavailable";
    state.infrastructureError = state.infrastructureUnavailable ? null : error.message;
  } finally {
    if (button) button.disabled = false;
  }
}

// ------------------------------------------------------------------ pieces

function row(label, value, mono) {
  const v = value === MISSING ? `<span class="infra-missing">${MISSING}</span>` : esc(value);
  return `<div class="infra-fact"><dt>${esc(label)}</dt><dd${mono ? ' class="mono"' : ""}>${v}</dd></div>`;
}

function diskBar(disk) {
  const pct = diskPercent(disk);
  if (pct === null) return row("Disk", MISSING);
  const used = fmtBytes(disk.total_bytes - disk.free_bytes);
  const total = fmtBytes(disk.total_bytes);
  const free = fmtBytes(disk.free_bytes);
  return `<div class="infra-fact infra-disk"><dt>Disk${disk.mount ? ` <code>${esc(disk.mount)}</code>` : ""}</dt>
    <dd>
      <div class="infra-meter" role="meter" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${pct}"
        aria-label="Disk ${pct}% used">
        <div class="infra-meter-fill" data-level="${diskLevel(pct)}" style="width:${pct}%"></div>
      </div>
      <div class="sub">${esc(used)} of ${esc(total)} used · ${esc(free)} free · ${pct}%</div>
    </dd>
  </div>`;
}

function hostCard(host) {
  const h = host || {};
  const title = h.hostname || "This machine";
  return `<article class="infra-card infra-host" aria-labelledby="infra-host-name">
    <header class="infra-card-head">
      <h3 id="infra-host-name">${esc(title)}</h3>
      <span class="tag">host</span>
    </header>
    <dl class="infra-facts">
      ${row("Model", fact(h.model))}
      ${row("Chip", fact(h.chip))}
      ${row("Cores", fact(h.cores))}
      ${row("Memory", fmtBytes(h.memory_bytes))}
      ${row("OS", fact(h.os))}
      ${row("Arch", fact(h.arch))}
      ${row("Uptime", fmtUptime(h.uptime_seconds))}
      ${row("Load 1 · 5 · 15m", fmtLoad(h.load), true)}
      ${diskBar(h.disk)}
    </dl>
  </article>`;
}

function daemonCard(daemon) {
  const d = daemon || {};
  const store = d.store || null;
  const storeText = store
    ? [fact(store.kind), fact(store.path), fmtBytes(store.size_bytes)].join(" · ")
    : MISSING;
  return `<article class="infra-card infra-daemon" aria-labelledby="infra-daemon-name">
    <header class="infra-card-head">
      <h3 id="infra-daemon-name">factory-daemon</h3>
      <span class="tag">daemon</span>
      ${d.version ? `<span class="sub">v${esc(d.version)}</span>` : ""}
    </header>
    <dl class="infra-facts">
      ${row("PID", fact(d.pid), true)}
      ${row("Uptime", fmtUptime(daemonUptime(d.started_at)))}
      ${row("Root", fact(d.root), true)}
      ${row("Store", storeText, true)}
      ${row("Socket", fact(d.socket), true)}
      ${row("Interfaces", fmtInterfaces(d.interfaces), true)}
      ${row("Runtime", fmtRuntime(daemon))}
    </dl>
  </article>`;
}

function agentLink(a) {
  const via = a.via === "agent"
    ? ` <span class="tag role" title="the agent's own provider: chose this account">own</span>`
    : "";
  return `<li><a href="${esc(agentHref(a.scope))}" title="open ${esc(a.scope)} on the roster">${esc(a.agent)}</a>
    <span class="sub">${esc(a.harness)}</span>${via}</li>`;
}

function agentGroups(agents) {
  return groupAgentsByScope(agents).map(g => `<div class="infra-scope">
      <div class="infra-scope-name">${esc(g.scope)}</div>
      <ul>${g.agents.map(agentLink).join("")}</ul>
    </div>`).join("");
}

function providerCard(p) {
  const all = p.agents || [];
  const shown = visibleAgents(all, inScope);
  const kind = kindBadge(p.kind);
  const agents = shown.length
    ? agentGroups(shown)
    : `<div class="idle">${all.length ? "No agent in the selected scope uses it." : "No agent uses it yet."}</div>`;
  return `<article class="infra-card infra-provider" data-kind="${esc(p.kind)}">
    <header class="infra-card-head">
      <h3>${esc(p.name)}</h3>
      <span class="tag infra-kind" data-kind="${esc(p.kind)}">${esc(kind)}</span>
      <span class="sub">${esc(fact(p.vendor))}</span>
    </header>
    <dl class="infra-facts">
      ${p.plan ? row("Plan", p.plan) : ""}
      ${p.env
        ? `<div class="infra-fact"><dt>Key in</dt><dd><code>${esc(p.env)}</code>
            <span class="sub">the variable's name; its value is never read</span></dd></div>`
        : ""}
      <div class="infra-fact"><dt>Agents</dt><dd class="sub">${all.length}</dd></div>
    </dl>
    <div class="infra-agents">${agents}</div>
  </article>`;
}

function unassignedBlock(rows) {
  const shown = visibleAgents(rows, inScope);
  if (!shown.length) return "";
  return `<article class="infra-card infra-unassigned">
    <header class="infra-card-head">
      <h3>Unassigned</h3>
      <span class="tag warn">${shown.length} agent${shown.length === 1 ? "" : "s"}</span>
    </header>
    <p class="infra-hint">These agents run a model harness, but no provider claims them. Declare one under
      <code>infrastructure.providers</code> in the root <code>.factory/config.yaml</code>, or give the agent a
      <code>provider:</code> of its own.</p>
    <div class="infra-agents">${agentGroups(shown)}</div>
  </article>`;
}

function emptyProviders() {
  return `<article class="infra-card infra-empty">
    <header class="infra-card-head"><h3>No AI accounts declared</h3></header>
    <p class="infra-hint">Factory never looks for accounts in credential files -- it only knows the ones the
      root config declares. Add a block like this to <code>.factory/config.yaml</code> and every agent on
      those harnesses is shown against its account here:</p>
    <pre class="infra-snippet"><code>${esc(PROVIDERS_SNIPPET)}</code></pre>
  </article>`;
}

function layer(label, sub, body) {
  return `<section class="infra-layer" aria-label="${esc(label)}">
    <div class="infra-layer-label"><span>${esc(label)}</span><span class="sub">${esc(sub)}</span></div>
    <div class="infra-layer-body">${body}</div>
  </section>`;
}

const RUNS_ON = `<div class="infra-on" aria-hidden="true"><span>runs on</span></div>`;

// ------------------------------------------------------------------ the view

export function renderInfrastructure() {
  const unavailable = $("infrastructure-unavailable");
  if (unavailable) unavailable.hidden = !state.infrastructureUnavailable;
  const failed = $("infrastructure-error");
  if (failed) {
    failed.textContent = state.infrastructureError || "";
    failed.hidden = !state.infrastructureError;
  }
  const stack = $("infrastructure");
  if (!stack) return;
  const data = state.infrastructure;
  if (!data) {
    stack.innerHTML = "";
    stack.hidden = true;
    return;
  }
  stack.hidden = false;

  const providers = isEmptyProviders(data)
    ? emptyProviders()
    : `<div class="infra-providers">${data.providers.map(providerCard).join("")}</div>`;
  const accounts = providers + unassignedBlock(data.unassigned);

  // Top to bottom on screen is the dependency read bottom to top: what the
  // agents pay through, then the daemon that runs them, then the machine
  // under it. The DOM follows the same order, so a screen reader and the tab
  // key meet the cards in the order they are drawn.
  stack.innerHTML = [
    layer("AI accounts", "what the agents' harnesses pay through", accounts),
    RUNS_ON,
    layer("Daemon", "Factory, and its store", daemonCard(data.daemon)),
    RUNS_ON,
    layer("Host", "the machine underneath", hostCard(data.host)),
  ].join("");
}
