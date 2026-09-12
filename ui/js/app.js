//! The wiring: which view is showing, and what an event from the daemon means
//! for it. Every module below is a view or a piece of one; this is the only
//! file that knows about all of them.

import { $, api, state, connect, setTheme, currentTheme, toggleTheme } from "./core.js";
import { initRail, writeHash } from "./scopes.js";
import { closeModal } from "./modal.js";
import { renderTasks, renderModal, loadJournal, retimeTerminal } from "./tasks.js";
import { loadAgents, renderAgents } from "./agents.js";
import { loadOccupancy, renderOccupancy } from "./occupancy.js";
import { openCreate } from "./task-form.js";
import { loadDashboard, renderDashboard, wireDashboard } from "./dashboard.js";
import { initActivity, recordEvent, markWatching, renderActivity } from "./activity.js";
import { showSite, hideSite, refreshSite } from "./site.js";

// ------------------------------------------------------------------ views
//
// Five entries, not two: `showTab` used to toggle exactly two `hidden`
// containers and two button classes. It is a small registry now, but the
// rule is the same -- one view visible, one button lit, and whatever that
// view needs to start or stop doing while it is not the one on screen.

let activityStarted = false;

const VIEWS = {
  dashboard: { onShow: loadDashboard },
  activity: {
    onShow: () => { if (!activityStarted) { initActivity(); activityStarted = true; } },
  },
  site: { onShow: () => refreshScopesThenSite(true), onHide: hideSite },
  tasks: { onShow: () => {} }, // state.tasks is already current; nothing to fetch
  agents: { onShow: () => showAgentView(state.agentView), onHide: stopAgentPoll },
};

// ------------------------------------------------------------------- scope

/// What the rail does when the selection changes. Nothing is refetched: every
/// view already holds the whole answer and the scope only decides how much of it
/// is drawn. Re-render, never reload -- `loadAgents` rebuilds the rail, and a
/// reload here would send it straight round again.
function rerender(route) {
  // Back and forward move the tab as well as the selection. The rail hands the
  // route over rather than reaching into the view, because which view is showing
  // is the page's business.
  if (route && route.tab !== state.tab) showTab(route.tab);
  // Unlike the other views, the dashboard's history cards are scoped on the
  // server (`/api/production` takes a scope), not just re-drawn narrower --
  // so a rail change has to refetch, not merely re-render.
  if (state.tab === "dashboard") { loadDashboard(); return; }
  // Everything already in the tail is still there; a scope change only
  // changes how much of it is drawn, the same re-render `renderTasks` does
  // below for the tasks it already holds.
  if (state.tab === "activity") { renderActivity(); return; }
  if (state.tab === "site") { refreshSite(); return; }
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
  if (!VIEWS[name]) name = "dashboard";
  const prev = state.tab;
  if (prev !== name && VIEWS[prev] && VIEWS[prev].onHide) VIEWS[prev].onHide();
  state.tab = name;
  writeHash(name);
  for (const k of Object.keys(VIEWS)) {
    $(`view-${k}`).hidden = k !== name;
    $(`tab-${k}`).classList.toggle("on", k === name);
  }
  VIEWS[name].onShow();
}

// The Agents view keeps its own two-way switch (Occupancy / Roster), each
// with a poll of its own -- ticking the now-line, or the elapsed times.
function stopAgentPoll() {
  if (state.agentPoll) { clearInterval(state.agentPoll); state.agentPoll = null; }
}

function showAgentView(view) {
  state.agentView = view;
  $("view-occupancy").hidden = view !== "occupancy";
  $("agents").hidden = view !== "roster";
  $("occ-window").hidden = view !== "occupancy";
  for (const b of $("agent-view").querySelectorAll("button")) {
    b.classList.toggle("on", b.dataset.view === view);
  }
  stopAgentPoll();
  if (view === "occupancy") {
    loadOccupancy();
    state.agentPoll = setInterval(loadOccupancy, 10000);
  } else {
    loadAgents();
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
    // `available` is served once, alongside the scopes rather than copied
    // onto each of them -- see `Payload::Scopes` -- so it is read from the
    // board itself, not from `scopes[0]` any more.
    const board = await api("/api/agents");
    state.scopes = board.scopes;
    state.adapters = board.available;
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

  for (const k of Object.keys(VIEWS)) {
    $(`tab-${k}`).onclick = () => showTab(k);
  }
  for (const b of $("agent-view").querySelectorAll("button")) {
    b.onclick = () => showAgentView(b.dataset.view);
  }
  $("occ-window").onchange = () => loadOccupancy();
  $("newTask").onclick = () => openCreate();
  wireDashboard();

  showTab("dashboard");

  connect({
    snapshot: (tasks) => { state.tasks = new Map(tasks.map(t => [t.id, t])); renderTasks(); if (state.tab === "dashboard") renderDashboard(); },
    event: onEvent,
    open: markWatching,
  });
}

boot();

// ------------------------------------------------------------------- events

/// What an event means for what is on screen. This is the one place that knows
/// every view, which is why it lives in the wiring and not in the transport.
function onEvent(ev) {
  recordEvent(ev);

  switch (ev.type) {
    case "task_created":
    case "task_updated":
      state.tasks.set(ev.task.id, ev.task);
      renderTasks();
      if (state.open === ev.task.id) renderModal();
      if (state.tab === "dashboard") renderDashboard();
      break;
    case "task_deleted":
      state.tasks.delete(ev.id);
      renderTasks();
      if (state.open === ev.id) closeModal();
      if (state.tab === "dashboard") renderDashboard();
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
  // The Agents and Site views are both a read over runs and standing agents;
  // either can change out from under them without a task event at all.
  if (ev.type.startsWith("run_") || ev.type.startsWith("agent_")) {
    if (state.tab === "agents") {
      if (state.agentView === "occupancy") loadOccupancy(); else loadAgents();
    }
    if (state.tab === "site") refreshScopesThenSite();
  }
  // A run reaching a terminal state is the one event that can change what
  // `/api/production` answers -- the dashboard's history cards refetch on it
  // rather than waiting for the window or scope to change.
  if (ev.type === "run_updated" && state.tab === "dashboard") loadDashboard();
}

/// The site's halls are built from `state.scopes`, which only the Agents view
/// otherwise keeps current. Pull a fresh copy before redrawing rather than
/// let the site quietly fall behind whenever nobody has the Agents tab open.
/// `opening` also runs the first-load path (footprint fetch, camera fit).
async function refreshScopesThenSite(opening) {
  try {
    state.scopes = (await api("/api/agents")).scopes;
  } catch { /* keep drawing with what we had */ }
  if (opening) showSite(); else refreshSite();
}
