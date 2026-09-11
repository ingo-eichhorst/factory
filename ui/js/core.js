//! The vocabulary every page shares: the DOM helpers, the client state, the
//! HTTP call and the socket. Nothing here knows that a task page or an agents
//! page exists -- `connect` hands events back to whoever wired it up.

export const $ = (id) => document.getElementById(id);
export const esc = (s) => String(s ?? "").replace(/[&<>"]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]));
export const TERMINAL = ["done", "failed", "cancelled"];

export const state = {
  tab: "tasks",
  tasks: new Map(),
  scopes: [],          // the agents page: scopes, each with its agents
  adapters: [],        // every agent adapter name, for the create form
  scopeNames: [],
  open: null,          // task id shown in the modal
  runs: [],            // runs of the open task
  run: null,           // selected run id
  term: null,          // {kind:'run'|'agent', id} the terminal is showing
  poll: null,          // terminal poll timer, for a transcript that is not live
  termSocket: null,    // the live terminal's socket, while one is open
  agentPoll: null,
  agentView: "occupancy", // the agents page: the chart, or the roster with its controls
  occ: null,              // the occupancy answer, as the daemon assembled it
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
/// task list the daemon sends on connect, `handlers.event` every event after.
export function connect(handlers) {
  const ws = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`);
  ws.onopen = () => { $("dot").classList.add("live"); $("conn").textContent = "live"; };
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
