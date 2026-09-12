//! The wiring: which page is showing, and what an event from the daemon means
//! for it. Every module below is a page or a piece of one; this is the only
//! file that knows about all of them.

import { $, api, state, connect, setTheme, currentTheme, toggleTheme } from "./core.js";
import { closeModal } from "./modal.js";
import { renderTasks, renderModal, loadJournal, retimeTerminal, applyTasksView, currentTasksView, setTasksView } from "./tasks.js";
import { loadAgents, renderAgents } from "./agents.js";
import { loadOccupancy } from "./occupancy.js";
import { openCreate } from "./task-form.js";

// ------------------------------------------------------------------- tabs
function showTab(name) {
  state.tab = name;
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
  // Before anything loads, so a returning visitor never sees the other shape
  // flash up first.
  applyTasksView(currentTasksView());

  try {
    const info = (await api("/api/status")).status;
    $("instance").textContent = `${info.instance} · ${info.root}`;
    state.scopeNames = info.scopes || [];
  } catch (e) { $("instance").textContent = e.message; }

  try {
    state.scopes = (await api("/api/agents")).scopes;
    if (state.scopes.length) state.adapters = state.scopes[0].available;
  } catch { state.scopes = []; }

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
  for (const b of $("tasks-view").querySelectorAll("button")) {
    b.onclick = () => setTasksView(b.dataset.view);
  }
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
