//! The wiring: which page is showing, and what an event from the daemon means
//! for it. Every module below is a page or a piece of one; this is the only
//! file that knows about all of them.

import { $, api, state, connect, setTheme, currentTheme, toggleTheme } from "./core.js";
import { initRail, writeHash } from "./scopes.js";
import { closeModal } from "./modal.js";
import { renderTasks, renderModal, loadJournal, retimeTerminal } from "./tasks.js";
import { loadAgents, renderAgents } from "./agents.js";
import { loadOccupancy, renderOccupancy } from "./occupancy.js";
import { openCreate } from "./task-form.js";

// ------------------------------------------------------------------- scope

/// What the rail does when the selection changes. Nothing is refetched: every
/// page already holds the whole answer and the scope only decides how much of it
/// is drawn. Re-render, never reload -- `loadAgents` rebuilds the rail, and a
/// reload here would send it straight round again.
function rerender(route) {
  // Back and forward move the tab as well as the selection. The rail hands the
  // route over rather than reaching into the page, because which tab is showing
  // is the page's business.
  if (route && route.tab !== state.tab) showTab(route.tab);
  if (state.tab === "tasks") { renderTasks(); return; }
  if (state.agentView === "occupancy") renderOccupancy(); else renderAgents();
}

/// The rail is a view over `state.scopes`, so it is rebuilt wherever that is
/// refreshed -- here at boot and in `loadAgents`, which is the other place that
/// refetches /api/agents. Exported so that one does not have to know what the
/// rail has to be rebuilt with.
export function rebuildRail() {
  initRail(rerender);
}

// ------------------------------------------------------------------- tabs
function showTab(name) {
  state.tab = name;
  writeHash(name);
  $("view-tasks").hidden = name !== "tasks";
  $("view-agents").hidden = name !== "agents";
  $("tab-tasks").classList.toggle("on", name === "tasks");
  $("tab-agents").classList.toggle("on", name === "agents");
  if (state.agentPoll) { clearInterval(state.agentPoll); state.agentPoll = null; }
  if (name === "agents") {
    showAgentView(state.agentView);
  }
}

function showAgentView(view) {
  state.agentView = view;
  $("view-occupancy").hidden = view !== "occupancy";
  $("agents").hidden = view !== "roster";
  $("occ-window").hidden = view !== "occupancy";
  for (const b of $("agent-view").querySelectorAll("button")) {
    b.classList.toggle("on", b.dataset.view === view);
  }
  if (state.agentPoll) { clearInterval(state.agentPoll); state.agentPoll = null; }
  if (view === "occupancy") {
    loadOccupancy();
    // The chart is a clock as much as a record: the now line has to move even
    // when nothing happens, and a running block has to keep growing.
    state.agentPoll = setInterval(loadOccupancy, 10000);
  } else {
    loadAgents();
    // Elapsed times tick even when nothing happens.
    state.agentPoll = setInterval(renderAgents, 5000);
  }
}

// ---------------------------------------------------------------------- boot

async function boot() {
  try {
    const info = (await api("/api/status")).status;
    $("instance").textContent = `${info.instance} · ${info.root}`;
    state.scopeNames = info.scopes || [];
    // A scope's served path is absolute, and the rail wants it the way the
    // config wrote it. This is the only endpoint that says where the instance
    // is, and it answers before the rail is built. Unanswered leaves the paths
    // whole, which draws a deeper tree but never a wrong one.
    state.root = info.root || "";
  } catch (e) { $("instance").textContent = e.message; }

  try {
    state.scopes = (await api("/api/agents")).scopes;
    if (state.scopes.length) state.adapters = state.scopes[0].available;
  } catch { state.scopes = []; }

  // /api/agents is the only endpoint that carries a scope's path, so the tree
  // cannot be built before it has answered. The rail reads the hash on the way
  // up, which is why the selection is in place before anything below draws.
  rebuildRail();

  try {
    const tasks = (await api("/api/tasks")).tasks;
    state.tasks = new Map(tasks.map(t => [t.id, t]));
    renderTasks();
  } catch { /* the websocket snapshot will fill it in */ }

  // The head already applied the theme; this puts its name on the switch.
  setTheme(currentTheme());
  $("theme").onclick = () => toggleTheme();

  $("tab-tasks").onclick = () => showTab("tasks");
  $("tab-agents").onclick = () => showTab("agents");
  for (const b of $("agent-view").querySelectorAll("button")) {
    b.onclick = () => showAgentView(b.dataset.view);
  }
  $("occ-window").onchange = () => loadOccupancy();
  $("newTask").onclick = () => openCreate();

  connect({
    snapshot: (tasks) => { state.tasks = new Map(tasks.map(t => [t.id, t])); renderTasks(); },
    event: onEvent,
  });
}

boot();

// ------------------------------------------------------------------- events

/// What an event means for what is on screen. This is the one place that knows
/// every page, which is why it lives in the wiring and not in the transport.
function onEvent(ev) {
  switch (ev.type) {
    case "task_created":
    case "task_updated":
      state.tasks.set(ev.task.id, ev.task);
      renderTasks();
      if (state.open === ev.task.id) renderModal();
      break;
    case "task_deleted":
      state.tasks.delete(ev.id);
      renderTasks();
      if (state.open === ev.id) closeModal();
      break;
    case "task_entry":
      if (state.open === ev.id) loadJournal();
      break;
    case "run_started":
    case "run_updated":
      if (state.open === ev.run.task_id) {
        const i = state.runs.findIndex(r => r.id === ev.run.id);
        if (i >= 0) state.runs[i] = ev.run; else state.runs.unshift(ev.run);
        // A new run is the one worth watching.
        if (ev.type === "run_started") state.run = ev.run.id;
        renderModal();
        retimeTerminal();
      }
      break;
  }
  // The agents page is a view over runs and standing agents alike.
  if (state.tab === "agents" && (ev.type.startsWith("run_") || ev.type.startsWith("agent_"))) {
    if (state.agentView === "occupancy") loadOccupancy(); else loadAgents();
  }
}
