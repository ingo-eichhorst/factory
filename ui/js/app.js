//! The wiring: which view is showing, and what an event from the daemon means
//! for it. Every module below is a view or a piece of one; this is the only
//! file that knows about all of them.

import { $, api, state, connect, setTheme, currentTheme, toggleTheme } from "./core.js";
import { initRail, writeHash, setRouter, readHash, applyRoute } from "./scopes.js";
import { closeModal, dropModal } from "./modal.js";
import { openTask, renderTasks, renderModal, loadJournal, retimeTerminal, applyTasksView, currentTasksView, setTasksView } from "./tasks.js";
import { loadAgents, renderAgents } from "./agents.js";
import { loadOccupancy, renderOccupancy } from "./occupancy.js";
import { legacyAgentRoute, loadRuntimeConnections, renderRuntimeConnections } from "./agent-runtime.js";
import { openCreate } from "./task-form.js";
import { acceptWorkflowEvent, loadWorkflows, readWorkflowTail, renderWorkflows, wireWorkflows, workflowTail } from "./workflows.js";
import { loadDashboard, renderDashboard, renderInbox, wireDashboard } from "./dashboard.js";
import { initActivity, recordEvent, markWatching, renderActivity, activityFilter, setActivityFilter } from "./activity.js";
import { showSite, hideSite, refreshSite, siteMode, setSiteMode, loadFootprint } from "./site.js";

// ------------------------------------------------------------------ views
//
// Eight entries, not two: `showTab` used to toggle exactly two `hidden`
// containers and two button classes. It is a small registry now, but the
// rule is the same -- one view visible, one button lit, and whatever that
// view needs to start or stop doing while it is not the one on screen.
//
// `tail` is the rest of the URL, for the views that have somewhere further to
// be than themselves. `write` says what the view is showing, in segments; `read`
// puts a link's segments back. Both are the view's own vocabulary -- the router
// carries the array and never looks in it.
//
let activityStarted = false;

const VIEWS = {
  dashboard: { onShow: loadDashboard },
  inbox: { onShow: () => renderInbox([...state.tasks.values()]) },
  activity: {
    onShow: () => { if (!activityStarted) { initActivity(); activityStarted = true; } },
    tail: { write: activityFilter, read: ([f]) => setActivityFilter(f || "all") },
  },
  site: {
    onShow: startSite,
    onHide: stopSite,
    tail: { write: siteMode, read: ([m]) => setSiteMode(m) },
  },
  tasks: { onShow: () => {} }, // state.tasks is already current; nothing to fetch
  workflows: {
    onShow: loadWorkflows,
    tail: { write: workflowTail, read: readWorkflowTail },
  },
  occupancy: {
    onShow: startOccupancy,
    onHide: stopAgentPoll,
  },
  roster: { onShow: startRoster, onHide: stopAgentPoll },
  "agent-runtime": { onShow: startAgentRuntime, onHide: stopAgentPoll },
};

// ------------------------------------------------------------------- the URL
//
// The router owns `#<scope>/<page>`; everything after it is composed here,
// because this is the only file that knows what all the views are.

/// Where the view's own tail stops and the open task's begins. A task is not a
/// property of one view -- it opens over the dashboard, the roster, the
/// occupancy chart and a hall on the site plan as readily as over the task list
/// -- so it is a layer on any page rather than a sixth tail, and it needs a
/// segment nobody can mistake for a view's own. No view writes `task`.
const MODAL = "task";

function viewTail(page) {
  const t = VIEWS[page] && VIEWS[page].tail;
  return t ? t.write() : [];
}

setRouter({
  pages: Object.keys(VIEWS),
  // Links written while these three screens lived behind one Agents tab keep
  // working. The router consumes the old inner-view segment and writes the
  // equivalent peer page back into the hash.
  redirects: {
    agents: legacyAgentRoute,
  },
  tailOf: () => {
    const tail = viewTail(state.tab);
    if (!state.open) return tail;
    return [...tail, MODAL, state.open, ...(state.run ? [state.run] : [])];
  },
});

/// The view's own segments and the open task's, split apart.
function splitTail(tail) {
  const cut = tail.indexOf(MODAL);
  return cut < 0 ? [tail, []] : [tail.slice(0, cut), tail.slice(cut + 1)];
}

/// A whole route onto the page: the view, what it was showing, and the task over
/// it. Called only while the router is applying, so nothing written here reaches
/// the URL -- the router writes once, at the end, from what this left on screen.
function applyTail(page, tail) {
  const [view, modal] = splitTail(tail);
  if (page !== state.tab) {
    showTab(page, view);
  } else {
    const t = VIEWS[page].tail;
    if (t) t.read(view, true);
  }
  applyModal(modal);
}

function applyModal([taskId, runId]) {
  if (!taskId) {
    if (state.open) dropModal();
    return;
  }
  // Back and forward land here on every step through a task's runs. Re-opening
  // the task that is already open would refetch its runs and its journal and
  // lose the terminal mid-stream.
  if (taskId === state.open && (!runId || runId === state.run)) return;
  openTask(taskId, runId);
}

// --------------------------------------------------------------- primary menu
//
// Dashboard is a peer of Factory's six decision levels in the first row. Its
// four operational views live together beneath it; the two implemented levels
// keep the view specific to each. This single map drives the second row, menu
// switching and the hash fallback in `scopes.js`.
const LEVEL_VIEWS = {
  dash: ["dashboard", "site", "activity", "inbox"],
  proc: ["tasks", "workflows"],
  harn: ["occupancy", "roster", "agent-runtime"],
};

/// The live level that claims `tab`, for backfilling `state.level` before any
/// hash has been read -- `scopes.js` keeps the same lookup for the hash
/// itself, but it works from a copy of this map handed in through `initRail`,
/// not from this function.
function levelForTab(tab) {
  for (const level of Object.keys(LEVEL_VIEWS)) {
    if (LEVEL_VIEWS[level].includes(tab)) return level;
  }
  return null;
}

/// Light the selected level button and nothing else.
function renderLevels() {
  const row = $("levels");
  if (!row) return;
  for (const b of row.querySelectorAll(".lvl")) {
    b.classList.toggle("on", b.dataset.level === state.level);
  }
}

/// Show only the second-row tabs that belong to the selected level. A
/// disabled level cannot reach this -- its buttons carry `disabled` and never
/// fire a click -- so `LEVEL_VIEWS` only ever needs the two live levels.
function renderTabRow() {
  const views = LEVEL_VIEWS[state.level] || [];
  for (const k of Object.keys(VIEWS)) {
    $(`tab-${k}`).hidden = !views.includes(k);
  }
}

/// What clicking a level button does: light it, swap the second row to its
/// views, and land on the first of them if the tab on screen is not one --
/// the same rule a hash-driven level change follows in `rerender`.
function setLevel(level) {
  if (level === state.level || !LEVEL_VIEWS[level]) return;
  state.level = level;
  renderLevels();
  renderTabRow();
  const views = LEVEL_VIEWS[level];
  if (!views.includes(state.tab)) showTab(views[0]);
  else writeHash();
}

// ------------------------------------------------------------------- scope

/// What the rail does when the selection changes. Nothing is refetched: every
/// view already holds the whole answer and the scope only decides how much of it
/// is drawn. Re-render, never reload -- `loadAgents` rebuilds the rail, and a
/// reload here would send it straight round again.
function rerender(route) {
  // Back and forward move the level as well as the page and the scope
  // selection, and the level goes first: the second tab row is filtered by it,
  // and a page applied into a row that does not list it would be shown and
  // hidden in the same tick. `route.level` is only ever present when the change
  // came from the hash (a plain rail click carries none), and re-lighting the
  // row and re-filtering the second one is cheap enough to do unconditionally
  // rather than compare against what is already there.
  if (route && route.level) {
    state.level = route.level;
    renderLevels();
    renderTabRow();
  }
  // The rail hands the route over rather than reaching into the view, because
  // which view is showing is the page's business. A null `tail` is the rail
  // rebuilding itself, not a navigation: the URL already describes what is on
  // screen, and applying it again would re-open the task just closed.
  if (route && route.tail) applyTail(route.page, route.tail);
  else if (route && route.page !== state.tab) showTab(route.page);
  // Unlike the other views, the dashboard's history cards are scoped on the
  // server (`/api/production` takes a scope), not just re-drawn narrower --
  // so a rail change has to refetch, not merely re-render.
  if (state.tab === "dashboard") { loadDashboard(); return; }
  if (state.tab === "inbox") { renderInbox([...state.tasks.values()]); return; }
  // Everything already in the tail is still there; a scope change only
  // changes how much of it is drawn, the same re-render `renderTasks` does
  // below for the tasks it already holds.
  if (state.tab === "activity") { renderActivity(); return; }
  if (state.tab === "site") { refreshSite(); return; }
  if (state.tab === "tasks") { renderTasks(); return; }
  if (state.tab === "workflows") { renderWorkflows(); return; }
  if (state.tab === "occupancy") renderOccupancy();
  else if (state.tab === "roster") renderAgents();
  else if (state.tab === "agent-runtime") renderRuntimeConnections();
}

/// The rail is a view over `state.scopes`, so it is rebuilt wherever that is
/// refreshed -- here at boot and in `loadAgents`, which is the other place that
/// refetches /api/agents. Exported so that one does not have to know what the
/// rail has to be rebuilt with.
export function rebuildRail() {
  initRail(rerender, LEVEL_VIEWS);
}

// ------------------------------------------------------------------- tabs

/// `tail` is only passed when a link asked for one; a click on the tab itself
/// leaves the view showing whatever it was showing last. It is read before
/// `onShow` so the view starts up already pointed where the URL wants it,
/// instead of loading its default and then being moved.
function showTab(name, tail) {
  if (!VIEWS[name]) name = "dashboard";
  const prev = state.tab;
  if (prev !== name && VIEWS[prev] && VIEWS[prev].onHide) VIEWS[prev].onHide();
  state.tab = name;
  if (tail && VIEWS[name].tail) VIEWS[name].tail.read(tail, false);
  for (const k of Object.keys(VIEWS)) {
    $(`view-${k}`).hidden = k !== name;
    $(`tab-${k}`).classList.toggle("on", k === name);
  }
  VIEWS[name].onShow();
  // Last, not first: the hash is written from what is on screen, and until
  // `onShow` has run the view has not finished saying what that is.
  writeHash();
}

// Each live agent read gets its own cadence: the now-line, roster elapsed
// times, or the runtime connection.
function stopAgentPoll() {
  if (state.agentPoll) { clearInterval(state.agentPoll); state.agentPoll = null; }
}

/// A hall says two things, and only one of them announces itself. What Factory
/// is doing arrives as events and redraws the site the moment it changes; how
/// big a scope is changes when somebody commits, which fires no event Factory
/// will ever hear. So the site also ticks, slowly: without it, a page left
/// open on a quiet instance would keep drawing a hall at the size it was when
/// the tab was opened. The daemon caches the walk, so a tick that finds
/// nothing new costs a query, and `update` keeps the scene standing.
const SITE_TICK_MS = 60000;

function startSite() {
  refreshScopesThenSite(true);
  stopSite();
  state.sitePoll = setInterval(() => refreshScopesThenSite(), SITE_TICK_MS);
}

function stopSite() {
  if (state.sitePoll) { clearInterval(state.sitePoll); state.sitePoll = null; }
  hideSite();
}

function startOccupancy() {
  stopAgentPoll();
  loadOccupancy();
  state.agentPoll = setInterval(loadOccupancy, 10000);
}

function startRoster() {
  stopAgentPoll();
  loadAgents();
  state.agentPoll = setInterval(renderAgents, 5000);
}

function startAgentRuntime() {
  stopAgentPoll();
  loadRuntimeConnections();
  state.agentPoll = setInterval(loadRuntimeConnections, 30000);
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
  // Only Dashboard and the two live levels reach here with a working click -- the four
  // greyed ones carry `disabled` in the markup, and a disabled button never
  // fires one.
  for (const b of $("levels").querySelectorAll(".lvl")) {
    b.onclick = () => setLevel(b.dataset.level);
  }
  for (const b of $("tasks-view").querySelectorAll("button")) {
    b.onclick = () => setTasksView(b.dataset.view);
  }
  $("runtime-refresh").onclick = () => loadRuntimeConnections();
  $("occ-window").onchange = () => loadOccupancy();
  $("newTask").onclick = () => openCreate();
  wireDashboard();
  wireWorkflows();

  // The hash is the boot route. Read here rather than in `initRail`, which runs
  // before any of the wiring above: a view cannot be shown until it can work.
  // `showTab` unconditionally, even for the dashboard the page already has on
  // screen, because a view that is never shown is never loaded either.
  const route = readHash();
  // The level first, and before the route is applied: the second tab row is
  // filtered by it, so a page shown while the level still says otherwise would
  // land in a row that hides it. `initRail` has already taken the level off the
  // hash if it named one; this backfills the case where it did not, which is
  // every hash written before levels existed.
  if (!state.level) state.level = levelForTab(route ? route.page : state.tab);
  renderLevels();
  renderTabRow();
  const [view, modal] = splitTail(route ? route.tail : []);
  applyRoute(() => {
    showTab(route ? route.page : "dashboard", view);
    applyModal(modal);
  });

  connect({
    snapshot: (tasks) => {
      state.tasks = new Map(tasks.map(t => [t.id, t]));
      renderTasks();
      if (state.tab === "dashboard") renderDashboard();
      if (state.tab === "inbox") renderInbox(tasks);
    },
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
  if (ev.type.startsWith("workflow_")) acceptWorkflowEvent(ev);

  switch (ev.type) {
    case "task_created":
    case "task_updated":
      state.tasks.set(ev.task.id, ev.task);
      renderTasks();
      if (state.open === ev.task.id) renderModal();
      if (state.tab === "dashboard") renderDashboard();
      if (state.tab === "inbox") renderInbox([...state.tasks.values()]);
      break;
    case "task_deleted":
      state.tasks.delete(ev.id);
      renderTasks();
      if (state.open === ev.id) { dropModal(); writeHash(true); }
      if (state.tab === "dashboard") renderDashboard();
      if (state.tab === "inbox") renderInbox([...state.tasks.values()]);
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
  // Occupancy and roster are reads over runs and standing agents; either can
  // change out from under them without a task event at all.
  if (ev.type.startsWith("run_") || ev.type.startsWith("agent_")) {
    if (state.tab === "occupancy") loadOccupancy();
    else if (state.tab === "roster") loadAgents();
  }
  // The site draws queued work too -- the crates at a hall's door, and the
  // floors its scope's load lights -- so a task arriving, being taken or being
  // deleted changes what it shows even when no run has started yet. Not
  // `task_entry`, which is one event per line an agent writes: the journal
  // says nothing about how full a hall is.
  if (state.tab === "site" && (ev.type.startsWith("run_") || ev.type.startsWith("agent_")
      || ev.type === "task_created" || ev.type === "task_updated" || ev.type === "task_deleted")) {
    refreshScopesThenSite();
  }
  // A run reaching a terminal state is the one event that can change what
  // `/api/production` answers -- the dashboard's history cards refetch on it
  // rather than waiting for the window or scope to change.
  if (ev.type === "run_updated" && state.tab === "dashboard") loadDashboard();
}

/// The site's halls are built from `state.scopes`, which only the Roster view
/// otherwise keeps current, and from `/api/site`, which is the only thing that
/// knows how big each scope is and how much of it is working. Both, together,
/// in one wait: the figures outside a hall and the lights on it are the same
/// fact seen twice, and fetching them a moment apart is how they come to
/// disagree. The daemon caches the walk behind `/api/site`, so asking again on
/// every event costs a query rather than a tree walk.
///
/// `opening` also runs the first-load path (footprint fetch, camera fit).
async function refreshScopesThenSite(opening) {
  try {
    const [agents] = await Promise.all([api("/api/agents"), loadFootprint()]);
    state.scopes = agents.scopes;
  } catch { /* keep drawing with what we had */ }
  if (opening) showSite(); else refreshSite();
}
