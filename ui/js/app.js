//! The wiring: which view is showing, and what an event from the daemon means
//! for it. Every module below is a view or a piece of one; this is the only
//! file that knows about all of them.

import { $, api, state, connect, setTheme, currentTheme, toggleTheme, TERMINAL } from "./core.js";
import { initRail, writeHash, setRouter, readHash, applyRoute } from "./scopes.js";
import { closeModal, dropModal } from "./modal.js";
import { openTask, renderTasks, renderModal, loadJournal, retimeTerminal, applyTasksView, currentTasksView, setTasksView, loadTaskUsage } from "./tasks.js";
import { loadAgents, renderAgents } from "./agents.js";
import { loadOccupancy, renderOccupancy } from "./occupancy.js";
import { legacyAgentRoute, loadRuntimeConnections, renderRuntimeConnections } from "./agent-runtime.js";
import { loadRoles, wireRoles } from "./roles.js";
import { openCreate } from "./task-form.js";
import { acceptWorkflowEvent, loadWorkflows, readWorkflowTail, renderWorkflows, wireWorkflows, workflowTail } from "./workflows.js";
import { loadDashboard, renderDashboard, loadInbox, renderInbox, wireDashboard } from "./dashboard.js";
import { loadOperations, showOperations, hideOperations, wireOperations } from "./operations.js";
import { initActivity, recordEvent, markWatching, renderActivity, activityFilter, setActivityFilter } from "./activity.js";
import { showSite, hideSite, refreshSite, siteMode, setSiteMode, loadFootprint } from "./site.js";
import { loadEnvironment, renderSandboxes } from "./sandboxes.js";
import { renderSecrets } from "./secrets.js";
import { benchTail, loadBenchmarks, readBenchTail, renderBenchmarks, wireBenchmarkSegments } from "./benchmarks.js";
import { loadDatasets, renderDatasetsSegment, wireDatasets } from "./datasets.js";
import { acceptBenchRunEvent, loadBenchRuns, renderBenchRunsSegment, wireBenchRuns } from "./bench-runs.js";
import { loadKnowledge, renderKnowledge, knowledgeTail, readKnowledgeTail } from "./knowledge.js";
import { loadInfrastructure, renderInfrastructure } from "./infrastructure.js";
import { refreshBackup, renderBackup, wireBackup } from "./backup.js";
import { isBackupEvent } from "./backup-model.js";
import { loadPolicy, reloadPolicy, wirePolicy } from "./policy.js";
import { loadGoals, reloadGoals, wireGoals } from "./goals.js";
import { loadQuality, reloadQuality, wireQuality } from "./quality.js";
import { loadScenarios, reloadScenarios, wireScenarios } from "./scenarios.js";

// ------------------------------------------------------------------ views
//
// Fifteen entries, not two: `showTab` used to toggle exactly two `hidden`
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
  // The daemon's attention list, every scope -- the same exceptions the
  // Operations tab shows per scope (`#106`), so there is one list, filtered
  // two ways, and not two derivations that can disagree.
  inbox: { onShow: loadInbox },
  // No poll: a policy read is cheap (catalogues on disk, the knowledge index,
  // the attestations store -- see `Request::Policy`'s doc comment) and
  // changes only when somebody attests or withdraws, which arrives as
  // `policy_changed` (`onEvent` below), the same no-poll rule `roles.js`
  // already follows for the same reason.
  policy: { onShow: loadPolicy },
  // Same reasoning as Policy: two small YAML files and a handful of already-
  // cached metric computations, re-read on every request, and a change only
  // ever arrives as `goals_changed` (a check-in), not on a clock.
  goals: { onShow: loadGoals },
  // Same again: the profiles are small YAML files re-read on every request,
  // and every value a scenario is judged on is already computed elsewhere.
  // What can change it arrives on the socket (`onEvent` below).
  quality: { onShow: loadQuality },
  // Not poll-free the same way: `loadScenarios` itself makes a second fetch
  // (`GET /api/metrics?ids=…`, for the titles a signpost strip needs) and
  // seeds the driver panel with one `POST /api/scenarios/whatif` call, so
  // this is a heavier `onShow` than its two neighbours -- still no poll,
  // since nothing here changes on a clock either.
  scenarios: { onShow: loadScenarios },
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
  // No poll: every fact the report reads changes with a task, run, journal
  // or agent event, and `onEvent` refetches on those (`scheduleOpsRefresh`).
  // `onHide` is where "since you last looked" is written down.
  operations: { onShow: showOperations, onHide: hideOperations },
  occupancy: {
    onShow: startOccupancy,
    onHide: stopAgentPoll,
  },
  roster: { onShow: startRoster, onHide: stopAgentPoll },
  "agent-runtime": { onShow: startAgentRuntime, onHide: stopAgentPoll },
  roles: { onShow: startRoles, onHide: stopAgentPoll },
  sandboxes: { onShow: startEnvironment, onHide: stopAgentPoll },
  secrets: { onShow: startEnvironment, onHide: stopAgentPoll },
  benchmarks: {
    onShow: startBenchmarks,
    onHide: stopAgentPoll,
    tail: {
      write: () => benchTail(),
      // `showTab` only calls `onShow` (`startBenchmarks`) when the page
      // itself changes; a tail change while Benchmarks is already showing
      // -- a typed hash, a rewritten link, a segment switched by app.js
      // itself -- lands here alone (`applyTail`'s else branch). Calling
      // `startBenchmarks` again is exactly what a fresh show would have
      // done, and every one of its loads already settles its own selection
      // and self-renders, so repeating it once more here is a correction,
      // never wasted work of a kind this app does not already tolerate
      // (see workflows.js's own note on `onShow` and `tail.read` overlapping).
      read: (tail) => {
        readBenchTail(tail);
        startBenchmarks();
      },
    },
  },
  knowledge: {
    onShow: startKnowledge,
    onHide: stopAgentPoll,
    tail: { write: knowledgeTail, read: readKnowledgeTail },
  },
  infrastructure: { onShow: startInfrastructure, onHide: stopAgentPoll },
  // Polled like Infrastructure -- the destination is a disk that can be
  // unplugged, which fires no event -- and refetched on every `backup_*`
  // event (`onEvent` below), which the daemon's own job publishes too.
  backup: { onShow: startBackup, onHide: stopAgentPoll },
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
// four operational views live together beneath it; the five implemented
// levels below it keep the view specific to each -- all six are live now.
// This single map drives the second row, menu switching and the hash
// fallback in `scopes.js`.
const LEVEL_VIEWS = {
  dash: ["dashboard", "site", "activity", "inbox"],
  // Goals first, then Quality, then Policy: Direction reads vision, then
  // quality, then rules -- the long-term frame and this cycle's objectives,
  // how good the work has to be on the way, and only then the control
  // catalogue that holds the company to what it already committed to.
  dir: ["goals", "quality", "policy", "scenarios"],
  // The work first, then how it is running.
  proc: ["tasks", "workflows", "operations"],
  harn: ["occupancy", "roster", "agent-runtime", "roles"],
  env: ["sandboxes", "secrets"],
  imp: ["benchmarks", "knowledge"],
  infra: ["infrastructure", "backup"],
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
/// fire a click -- so `LEVEL_VIEWS` only ever needs the live levels.
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
  // The Inbox is every scope's, whatever the rail says -- nothing to redo.
  if (state.tab === "inbox") { renderInbox(); return; }
  // The daemon narrows the report to the selected subtree (`operations.js`'s
  // header), so a rail change refetches.
  if (state.tab === "operations") { loadOperations(); return; }
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
  // Not a re-render: the roles in effect, and who holds them, are a
  // different answer in every scope, and the daemon is what resolves it.
  else if (state.tab === "roles") loadRoles();
  // Same reason: which controls apply and their status differ by scope --
  // the daemon folds the chain itself (`GET /api/policy?scope=`), so a rail
  // change refetches rather than narrowing what is already on screen.
  else if (state.tab === "policy") loadPolicy();
  // Same reason again: which objectives and roadmap items belong to a scope
  // is the daemon's own filter (`GET /api/goals?scope=`).
  else if (state.tab === "goals") loadGoals();
  // Same reason again: which profiles bind a scope is its chain, folded by
  // the daemon (`GET /api/quality?scope=`).
  else if (state.tab === "quality") loadQuality();
  // Same reason again: `GET /api/scenarios?scope=` narrows the baseline and
  // every scenario's own policy delta to the asked subtree -- the one
  // exception is the driver panel's own `POST /api/scenarios/whatif`, which
  // carries no scope at all (`driverPanelHtml`'s own caption says so).
  else if (state.tab === "scenarios") loadScenarios();
  // Both L2 tabs answer to the rail. Secrets narrows only the rows that
  // belong to a scope: a credential in the owner's home belongs to none of
  // them and is reachable from all of them, so it survives every selection.
  else if (state.tab === "sandboxes") renderSandboxes();
  else if (state.tab === "secrets") renderSecrets();
  else if (state.tab === "benchmarks") {
    // The rail narrows a selected dataset's cases and a selected run's
    // attempts matrix; Configurations narrows its agents the same way it
    // always has. All three redraw from whatever they already hold -- no
    // view here answers to a scope-scoped fetch, unlike the dashboard above.
    renderBenchmarks();
    renderDatasetsSegment();
    renderBenchRunsSegment();
  }
  // Knowledge answers to no scope -- the knowledge base is company-wide --
  // so this redraws the same rows every time. Included anyway so the map
  // above stays a complete list of every tab rather than all-but-one.
  else if (state.tab === "knowledge") renderKnowledge();
  // The host, the daemon and the accounts are the whole instance's, whatever
  // the rail says; what narrows is the agents listed under each account and
  // under Unassigned -- the same split Secrets keeps for the home directory.
  else if (state.tab === "infrastructure") renderInfrastructure();
  // A backup is of the whole instance: no rail selection narrows it.
  else if (state.tab === "backup") renderBackup();
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

/// No poll: roles change only when somebody writes one or gives one, and
/// both of those arrive as events.
function startRoles() {
  stopAgentPoll();
  loadRoles();
}

function startAgentRuntime() {
  stopAgentPoll();
  loadRuntimeConnections();
  state.agentPoll = setInterval(loadRuntimeConnections, 30000);
}

/// Both L2 tabs read one answer, so both `onShow` handlers point here rather
/// than each fetching their own: `loadEnvironment` only sets state, and this
/// is the one place -- app.js, which already knows every view -- that renders
/// both of them from it.
async function refreshEnvironment() {
  await loadEnvironment();
  renderSandboxes();
  renderSecrets();
}

function startEnvironment() {
  stopAgentPoll();
  refreshEnvironment();
  state.agentPoll = setInterval(refreshEnvironment, 30000);
}

// L5's two tabs each read their own answer and neither needs a poll: a
// configuration only changes when someone edits `config.yaml`, and the wiki
// only changes when someone edits a file, so a fetch on show plus Refresh is
// the whole story -- `stopAgentPoll` still runs, to clear a poll left running
// by whichever view was on screen before.
//
// Benchmarks now has three segments, and each fetches its own answer:
// configurations and the (company-wide) dataset list are cheap enough to load
// unconditionally, so switching segments in-tab is instant; bench runs are
// fetched only once the Runs segment is actually the one showing, whether
// that came from a click (`onBenchSegmentSwitch`) or straight off the hash on
// boot or reload. `loadDatasets`/`loadBenchRuns` each settle their own
// selection against the fresh answer and load its detail, so a deep link to
// one dataset or run needs nothing further here.
function startBenchmarks() {
  stopAgentPoll();
  loadBenchmarks();
  loadDatasets();
  if (state.benchSegment === "runs") loadBenchRuns();
}

/// Fired when the segmented control switches to a segment this file has not
/// loaded yet -- `benchmarks.js` cannot import `datasets.js`/`bench-runs.js`
/// itself (see its header comment), so it hands the switch back here, the one
/// file that already knows every view.
function onBenchSegmentSwitch(seg) {
  if (seg === "datasets" && !state.datasets) loadDatasets();
  if (seg === "runs" && !state.benchRuns) loadBenchRuns();
}

function startKnowledge() {
  stopAgentPoll();
  loadKnowledge();
}

/// L1 reads one answer and renders one view from it, on the same thirty
/// seconds the L2 tabs poll at: the host's uptime, load and free disk move
/// without any event to announce it, and the daemon collects them fresh on
/// every request rather than caching them.
async function refreshInfrastructure() {
  await loadInfrastructure();
  renderInfrastructure();
}

function startInfrastructure() {
  stopAgentPoll();
  refreshInfrastructure();
  state.agentPoll = setInterval(refreshInfrastructure, 30000);
}

function startBackup() {
  stopAgentPoll();
  refreshBackup();
  state.agentPoll = setInterval(refreshBackup, 30000);
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
  // Only Dashboard and the five live levels reach here with a working click --
  // Every level is live, so every button gets a handler.
  for (const b of $("levels").querySelectorAll(".lvl")) {
    b.onclick = () => setLevel(b.dataset.level);
  }
  for (const b of $("tasks-view").querySelectorAll("button")) {
    b.onclick = () => setTasksView(b.dataset.view);
  }
  $("runtime-refresh").onclick = () => loadRuntimeConnections();
  wireRoles();
  wirePolicy();
  wireGoals();
  wireQuality();
  wireScenarios();
  $("environment-refresh").onclick = () => refreshEnvironment();
  $("secrets-refresh").onclick = () => refreshEnvironment();
  $("benchmarks-refresh").onclick = () => {
    loadBenchmarks();
    loadDatasets();
    if (state.benchSegment === "runs") loadBenchRuns();
  };
  wireBenchmarkSegments(onBenchSegmentSwitch);
  wireDatasets();
  wireBenchRuns();
  $("knowledge-refresh").onclick = () => loadKnowledge();
  $("infrastructure-refresh").onclick = () => refreshInfrastructure();
  wireBackup();
  $("occ-window").onchange = () => loadOccupancy();
  $("newTask").onclick = () => openCreate();
  wireDashboard();
  wireWorkflows();
  wireOperations();
  // A page in a background browser tab skips its refetches; coming back is
  // when it catches up, once.
  document.addEventListener("visibilitychange", () => { if (!document.hidden) scheduleOpsRefresh(); });

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
      scheduleOpsRefresh();
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
  // Read before the switch below overwrites it: the Quality tab reloads on a
  // `quality=` task's status *changing*, not on every progress update.
  const priorTask = ev.task ? state.tasks.get(ev.task.id) : null;
  recordEvent(ev);
  if (ev.type.startsWith("workflow_")) acceptWorkflowEvent(ev);

  switch (ev.type) {
    case "task_created":
    case "task_updated":
      state.tasks.set(ev.task.id, ev.task);
      renderTasks();
      if (state.open === ev.task.id) renderModal();
      if (state.tab === "dashboard") renderDashboard();
      // Judging is asynchronous: a bench attempt's task settles (this
      // event) well before its gate finishes and `bench_run_updated`
      // delivers the verdict. Without this, the selected run's matrix would
      // sit on "running" the whole time the gate is out, rather than
      // showing "judging" the moment the task itself is actually done.
      if (state.tab === "benchmarks" && ev.task.bench_origin && ev.task.bench_origin.bench_run_id === state.benchRunId) {
        renderBenchRunsSegment();
      }
      break;
    case "task_deleted":
      state.tasks.delete(ev.id);
      renderTasks();
      if (state.open === ev.id) { dropModal(); writeHash(true); }
      if (state.tab === "dashboard") renderDashboard();
      break;
    case "task_entry":
      if (state.open === ev.id) loadJournal();
      break;
    case "run_started":
    case "run_updated":
      if (state.open === ev.run.task_id) {
        const i = state.runs.findIndex(r => r.id === ev.run.id);
        // A new usage reading moves the task's sum too (#117); re-read it
        // only then, not on every status flicker.
        const usageMoved = JSON.stringify(i >= 0 ? state.runs[i].usage : null) !== JSON.stringify(ev.run.usage || null);
        if (i >= 0) state.runs[i] = ev.run; else state.runs.unshift(ev.run);
        if (usageMoved) loadTaskUsage().then(renderModal);
        // A new run is the one worth watching.
        if (ev.type === "run_started") state.run = ev.run.id;
        renderModal();
        retimeTerminal();
      }
      break;
    case "bench_run_updated":
      acceptBenchRunEvent(ev.run);
      break;
  }
  // Occupancy and roster are reads over runs and standing agents; either can
  // change out from under them without a task event at all.
  if (ev.type.startsWith("run_") || ev.type.startsWith("agent_")) {
    if (state.tab === "occupancy") loadOccupancy();
    else if (state.tab === "roster") loadAgents();
  }
  // A role written or removed changes what every scope below it has, and a
  // role given or a declaration changed changes who holds what. Not
  // `agent_activity`, which is a runtime saying an agent is busy, several
  // times a minute.
  if (ev.type === "roles_changed" || ev.type === "agent_updated" || ev.type === "agent_removed"
      || ev.type === "agent_configured" || ev.type === "agent_deleted") {
    if (state.tab === "roles") loadRoles();
    else if (state.tab === "roster" && ev.type === "roles_changed") loadAgents();
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
  // An attestation recorded or withdrawn, published on both -- see
  // `Event::PolicyChanged`. Reload whenever the tab is open, not only when
  // the scope it names is the one on screen: an ancestor's attestation can
  // change a descendant's rollup too, and `reloadPolicy` also refreshes the
  // control detail modal, if one happens to be open.
  if (ev.type === "policy_changed" && state.tab === "policy") reloadPolicy();
  // A check-in recorded against a manual key result -- see `Event::GoalsChanged`.
  // Reload whenever the tab is open: `reloadGoals` also refreshes the key
  // result detail modal, if one happens to be open on the checked-in key
  // result.
  if (ev.type === "goals_changed" && state.tab === "goals") reloadGoals();
  // A backup taken (by a person or the schedule), failed or verified.
  if (isBackupEvent(ev) && state.tab === "backup") refreshBackup();
  // A scenario's own policy delta and goal-scenario probabilities are read
  // off the same live evidence and check-ins those two events already name;
  // `reloadScenarios` is a full reload (the forecast itself is a Monte Carlo
  // draw the daemon has to recompute, not a client-side re-render), so this
  // deliberately does not also answer to `run_updated`/`task_created` --
  // Refresh covers those, the same restraint the Scenarios tab's own `onShow`
  // comment explains.
  if ((ev.type === "policy_changed" || ev.type === "goals_changed") && state.tab === "scenarios") reloadScenarios();
  // Three things move the Quality tab, and only while it is open:
  // `quality_changed` (a profile or a chain changed -- noticed on some read,
  // see `Event::QualityChanged`); a run reaching a terminal state, which is
  // when a fitness function's verdict lands and a production metric moves
  // (not every `run_updated` -- a run reporting progress changes nothing a
  // scenario reads); and a task carrying a `quality=` label appearing,
  // changing status or going away, which is `open_tasks` changing under a
  // scenario's "Create task" -- not every `task_updated` it sends, most of
  // which are progress. A deleted task's event carries only its id, so any
  // deletion reloads -- rare enough not to be worth telling apart.
  const qualityTask = ev.task && ev.task.labels && ev.task.labels.quality;
  if (state.tab === "quality" && (ev.type === "quality_changed"
      || (ev.type === "run_updated" && TERMINAL.includes(ev.run.status))
      || (ev.type === "task_created" && qualityTask)
      || (ev.type === "task_updated" && qualityTask && (!priorTask || priorTask.status !== ev.task.status))
      || ev.type === "task_deleted")) {
    reloadQuality();
  }
  // Every fact `/api/operations` reads arrives as one of these (PR #113 adds
  // no event of its own). Not `agent_activity`, a runtime saying an agent is
  // busy several times a minute, which changes nothing the report says.
  if ((ev.type.startsWith("task_") || ev.type.startsWith("run_") || ev.type.startsWith("agent_")) && ev.type !== "agent_activity") {
    scheduleOpsRefresh();
  }
}

/// One refetch of `/api/operations` for a burst of events, at most one per
/// `OPS_REFRESH_MS`, and none at all unless the Operations tab or the Inbox
/// is what is on screen in a browser tab someone can see. The unscoped read
/// walks sixty days of runs and the journal of every open one -- not free --
/// and `task_entry` fires once per line an agent writes. The first event of
/// a burst starts the clock and the rest ride along, so a steady stream
/// still refreshes every interval rather than never, as a trailing debounce
/// would.
const OPS_REFRESH_MS = 1500;
let opsTimer = null;

function scheduleOpsRefresh() {
  if (opsTimer) return;
  if (state.tab !== "operations" && state.tab !== "inbox") return;
  opsTimer = setTimeout(() => {
    opsTimer = null;
    if (document.hidden) return;
    if (state.tab === "operations") loadOperations();
    else if (state.tab === "inbox") loadInbox();
  }, OPS_REFRESH_MS);
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
