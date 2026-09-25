//! The vocabulary every page shares: the DOM helpers, the client state, the
//! HTTP call and the socket. Nothing here knows that a task or roster
//! page exists -- `connect` hands events back to whoever wired it up.

export const $ = (id) => document.getElementById(id);
export const esc = (s) => String(s ?? "").replace(/[&<>"]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]));
export const TERMINAL = ["done", "failed", "cancelled"];

export const state = {
  roles: [],           // the roles that hold in every scope
  scopeRoles: {},      // scope name -> its roles, where they differ from `roles`
  roleBoard: null,     // the last /api/roles answer, for the Roles view
  roleBoardError: null,
  tab: "dashboard",
  tasks: new Map(),
  scopes: [],          // configured scopes, each with its agents
  adapters: [],        // every agent adapter name, for the create form
  scopeNames: [],
  root: "",            // the instance root, so the rail can read a scope path
                       // as the config wrote it and not as the disk spells it
  scope: null,         // the rail's selection, by name; null is every scope
  level: null,         // the selected primary menu key ("dash", "proc", "harn", …);
                       // null until boot derives it from the tab, or a hash
                       // names one -- app.js's LEVEL_VIEWS is the one place
                       // that says which
  open: null,          // task id shown in the modal
  runs: [],            // runs of the open task
  run: null,           // selected run id
  term: null,          // {kind:'run'|'agent', id} the terminal is showing
  poll: null,          // terminal poll timer, for a transcript that is not live
  termSocket: null,    // the live terminal's socket, while one is open
  agentPoll: null,
  runtimeConnections: [], // one diagnostic per effective runtime connection
  runtimeConnectionError: null,
  sitePoll: null,        // the site view's slow tick, for the half of it no event announces
  occ: null,              // the occupancy answer, as the daemon assembled it
  environment: null,      // last /api/environment answer: { sandboxes, credentials }
  environmentError: null,
  infrastructure: null,   // last /api/infrastructure answer: { host, daemon, providers, unassigned }
  infrastructureError: null,
  infrastructureUnavailable: false, // the daemon predates the endpoint (a bare 404), not a fault
  benchmarks: null,       // last /api/benchmarks answer: { configurations }
  benchmarksError: null,
  knowledge: null,        // last /api/knowledge answer: { root, present, notes, gaps, pages, findings }
  knowledgeError: null,
  policy: null,           // last /api/policy answer's report -- see PolicyReport in protocol.rs
  policyError: null,

  // L6 Direction, Goals tab (#99 slice 3) -- see GoalsReport in protocol.rs.
  goals: null,             // last /api/goals answer's report
  goalsError: null,
  goalsCycle: null,        // the cycle id the rail/switcher asked for; null defers to the daemon's own default (the current cycle)
  goalsMetrics: null,      // last /api/metrics?ids=... answer: { values, series, registry } -- registry (unit) and series, for the ids goals-model.js's metricIdsForReport names

  // L5 Improvement, Benchmarks tab -- three segments over one shared state
  // object, the same pattern `sandboxes.js`/`secrets.js` already share for
  // L2's two tabs. `benchSegment` is which of the three is showing;
  // `benchDatasetName`/`benchRunId` are the selection within it, kept here
  // rather than module-private so datasets.js and bench-runs.js can read and
  // write the selection without importing one another.
  benchSegment: "datasets",   // "datasets" | "runs" | "configurations"
  benchDatasetName: null,     // selected dataset, when benchSegment === "datasets"
  benchRunId: null,           // selected bench run id, when benchSegment === "runs"
  datasets: null,             // last /api/datasets answer: { root, datasets }
  datasetsError: null,
  dataset: null,              // last /api/datasets/{name} answer: { dataset, findings }
  datasetError: null,
  benchRuns: null,            // last /api/bench/runs answer: { runs }
  benchRunsError: null,
  benchRun: null,             // last /api/bench/runs/{id} answer: { run, results }
  benchRunError: null,
};


// ---------------------------------------------------------------- transport

export async function api(path, opts) {
  const res = await fetch(path, { headers: { "content-type": "application/json" }, ...opts });
  const body = await res.json().catch(() => null);
  if (!body) throw new Error(`${res.status} ${res.statusText}`);
  if (body.status === "error") throw new Error(body.message);
  return body.data;
}

/// The socket, and nothing about what is on it. `handlers.snapshot` gets the
/// task list the daemon sends on connect, `handlers.event` every event after,
/// and the optional `handlers.open` fires the moment the socket is live --
/// which is the true start of "what this page has seen", for anything that
/// needs to say so honestly rather than counting from when its own view
/// happened to first be shown.
export function connect(handlers) {
  const ws = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`);
  ws.onopen = () => {
    $("dot").classList.add("live"); $("conn").textContent = "live";
    if (handlers.open) handlers.open();
  };
  ws.onclose = () => {
    $("dot").classList.remove("live"); $("conn").textContent = "reconnecting";
    setTimeout(() => connect(handlers), 1500);
  };
  ws.onmessage = (m) => {
    let msg; try { msg = JSON.parse(m.data); } catch { return; }
    if (msg.status !== "ok") return;
    const d = msg.data;
    if (d.kind === "tasks") handlers.snapshot(d.tasks);
    else if (d.kind === "event") handlers.event(d.event);
  };
}

// -------------------------------------------------------------- the theme

/// Two named themes, remembered per browser. The choice is applied by an
/// inline script in the page head before the first paint; all this does is
/// change it and write it down. A browser that refuses storage still gets a
/// working switch, it just forgets by the next visit.
const THEMES = { "foundry-dark": "foundry dark", "foundry-light": "foundry light" };

export function currentTheme() {
  const t = document.documentElement.dataset.theme;
  return THEMES[t] ? t : "foundry-dark";
}

export function setTheme(name) {
  const theme = THEMES[name] ? name : "foundry-dark";
  document.documentElement.dataset.theme = theme;
  try { localStorage.setItem("factory-theme", theme); } catch (e) { /* private window */ }
  const button = $("theme");
  if (button) button.textContent = THEMES[theme];
  // The site plan's two renderers read the palette through CSS custom
  // properties and cache it, so a switch has to tell them to read it again --
  // otherwise a scene already on screen keeps the colours it booted with.
  document.dispatchEvent(new CustomEvent("factory:theme"));
}

export function toggleTheme() {
  setTheme(currentTheme() === "foundry-dark" ? "foundry-light" : "foundry-dark");
}

// ------------------------------------------------------------ shared shapes

export function statusBadge(s) { return `<span class="badge s-${esc(s)}">${esc(s)}</span>`; }

export function since(iso) {
  const secs = Math.max(0, Math.round((Date.now() - new Date(iso).getTime()) / 1000));
  if (secs < 90) return `${secs}s`;
  if (secs < 5400) return `${Math.round(secs / 60)}m`;
  return `${Math.round(secs / 3600)}h`;
}

export function shortSpan(secs) {
  if (secs < 60) return `${Math.round(secs)}s`;
  if (secs < 5400) return `${Math.round(secs / 60)}m`;
  return `${(secs / 3600).toFixed(secs < 36000 ? 1 : 0)}h`;
}
