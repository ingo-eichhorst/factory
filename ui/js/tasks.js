//! The task list and the task modal: its runs, its journal, and the terminal of
//! whichever run is selected.

import { $, esc, api, state, since, statusBadge, TERMINAL } from "./core.js";
import { scrim, closeModal } from "./modal.js";
import { terminalBlock, wireTerminal, setTerminal } from "./terminal.js";
import { openEdit, scheduleText } from "./task-form.js";

export function scheduleLabel(s) {
  if (!s) return "manual";
  if (s.cron) return `cron ${s.cron}`;
  if (s.every) return `every ${s.every.seconds}s`;
  return "scheduled";
}

// -------------------------------------------------------------- the shape
// Kanban or table, remembered per browser the same way the theme is: a live
// value here, seeded once from `localStorage` and best-effort mirrored back
// to it. A browser that refuses storage still switches, it just forgets by
// the next visit.
const VIEW_KEY = "factory-tasks-view";
let tasksView = "board";
try { if (localStorage.getItem(VIEW_KEY) === "table") tasksView = "table"; }
catch (e) { /* private window */ }

export function currentTasksView() { return tasksView; }

/// Which container shows, and which button looks pressed. No data here --
/// that is `renderTasks`'s business, so this alone can run before the first
/// task ever loads.
export function applyTasksView(view) {
  tasksView = view === "table" ? "table" : "board";
  $("tasks-table").hidden = tasksView !== "table";
  $("kanban").hidden = tasksView !== "board";
  for (const b of $("tasks-view").querySelectorAll("button")) b.classList.toggle("on", b.dataset.view === tasksView);
}

export function setTasksView(view) {
  applyTasksView(view);
  try { localStorage.setItem(VIEW_KEY, tasksView); } catch (e) { /* private window */ }
  renderTasks();
}

// -------------------------------------------------------------- the board
// Columns are `TaskStatus` and nothing else. Blocked is first because it is
// the one column a person has to act on; pending splits by how it will
// start, so a scheduled task never sits in the same pile as a manual one;
// dispatching and running share a column because a session still opening is
// not yet work; done, failed and cancelled share a closed column, but each
// card says its outcome plainly so a failure cannot read as done.
const KANBAN_COLUMNS = [
  { key: "blocked", label: "Blocked" },
  { key: "manual", label: "Manual" },
  { key: "scheduled", label: "Scheduled" },
  { key: "active", label: "In progress" },
  { key: "closed", label: "Closed" },
];

function columnFor(t) {
  if (t.status === "blocked") return "blocked";
  if (t.status === "pending") return t.schedule ? "scheduled" : "manual";
  if (t.status === "dispatching" || t.status === "running") return "active";
  return "closed";
}

/// Only what `/api/tasks` already serves: title, short id, status, scope,
/// agent, the schedule rule or the run count, and how long the newest run
/// has been going. No estimate -- there is no template here to read one from.
function taskCard(t) {
  const bits = [];
  bits.push(t.schedule ? scheduleLabel(t.schedule) : (t.runs ? `${t.runs} run${t.runs === 1 ? "" : "s"}` : "no runs yet"));
  if (!TERMINAL.includes(t.status) && t.status !== "pending" && t.last_run_at) bits.push(since(t.last_run_at));
  return `
    <div class="kbc" data-id="${esc(t.id)}">
      <div class="kbc-top">${statusBadge(t.status)}<code class="id">${esc(t.id.slice(0, 8))}</code></div>
      <div class="title">${esc(t.title)}</div>
      <div class="sub">${esc(t.scope)} · ${esc(t.agent)}</div>
      <div class="sub">${esc(bits.join(" · "))}</div>
    </div>`;
}

function renderBoard(rows) {
  const byCol = new Map(KANBAN_COLUMNS.map(c => [c.key, []]));
  for (const t of rows) byCol.get(columnFor(t)).push(t);
  $("kanban").innerHTML = KANBAN_COLUMNS.map(c => {
    const items = byCol.get(c.key);
    return `
      <div class="kbcol" data-col="${c.key}">
        <div class="kbcol-head"><span>${esc(c.label)}</span><span class="kbcol-count">${items.length}</span></div>
        <div class="kbcol-body">${items.length ? items.map(taskCard).join("") : `<div class="kbcol-empty">—</div>`}</div>
      </div>`;
  }).join("");
  for (const el of $("kanban").querySelectorAll(".kbc")) {
    el.onclick = () => openTask(el.dataset.id);
  }
}

export function renderTasks() {
  const rows = [...state.tasks.values()].sort((a, b) => b.created_at.localeCompare(a.created_at));
  $("noTasks").hidden = rows.length > 0;
  $("tasks").innerHTML = rows.map(t => `
    <tr class="row" data-id="${esc(t.id)}">
      <td><div class="title">${esc(t.title)}</div>
          <div class="sub">${esc(scheduleLabel(t.schedule))}</div></td>
      <td>${statusBadge(t.status)}</td>
      <td class="sub">${t.runs || 0}</td>
      <td class="sub">${esc(t.scope)}</td>
      <td class="sub">${esc(t.agent)}</td>
    </tr>`).join("");
  for (const tr of $("tasks").querySelectorAll("tr.row")) {
    tr.onclick = () => openTask(tr.dataset.id);
  }

  renderBoard(rows);
  // Six empty columns say less than one sentence: with nothing to show, the
  // board steps aside for the same "Nothing here yet." the table already has.
  if (tasksView === "board") $("kanban").hidden = rows.length === 0;
}

export async function openTask(id, runId) {
  closeModal();
  state.open = id;
  state.run = runId || null;
  scrim(`
    <header>
      <div><h2 id="m-title">…</h2><code class="id" id="m-id"></code></div>
      <button class="x" id="m-close">&times;</button>
    </header>
    <div class="body">
      <div class="row-btns">
        <button class="btn" id="m-run">Run</button>
        <button class="btn" id="m-cancel">Cancel</button>
        <button class="btn" id="m-edit">Edit</button>
        <button class="btn danger" id="m-delete">Delete</button>
      </div>
      <div class="err" id="m-err"></div>
      <div id="m-meta"></div>
      <label>Runs</label>
      <div class="runs" id="m-runs"></div>
      <label>Journal</label>
      <div class="journal" id="m-journal"></div>
      ${terminalBlock("Terminal")}
    </div>`);

  $("m-close").onclick = closeModal;
  $("m-run").onclick = () => act(`/api/tasks/${state.open}/run`);
  $("m-cancel").onclick = () => act(`/api/tasks/${state.open}/cancel`);
  $("m-edit").onclick = () => { const task = state.tasks.get(state.open); if (task) openEdit(task); };
  $("m-delete").onclick = () => act(`/api/tasks/${state.open}`, "DELETE");
  wireTerminal();

  await loadRuns();
  renderModal();
  await loadJournal();
  retimeTerminal();
}

/// A standing agent's own window: what its terminal shows, and a way to answer
/// it. This is what makes a trust dialog or a login something you can get past

export async function act(path, method = "POST") {
  $("m-err").textContent = "";
  try { await api(path, { method }); }
  catch (e) { $("m-err").textContent = e.message; }
}

export async function loadRuns() {
  try {
    state.runs = (await api(`/api/tasks/${state.open}/runs?limit=50`)).runs;
    if (!state.run || !state.runs.some(r => r.id === state.run)) {
      state.run = state.runs.length ? state.runs[0].id : null;
    }
  } catch { state.runs = []; }
}

/// Point the terminal at whichever run is selected. Lives here and not in
/// terminal.js: which run that is, is the modal's business.
export function retimeTerminal() {
  const r = selectedRun();
  setTerminal("run", r ? r.id : null, r ? !TERMINAL.includes(r.status) : false);
}

export function selectedRun() { return state.runs.find(r => r.id === state.run) || null; }

export function renderModal() {
  const t = state.tasks.get(state.open);
  if (!t || !$("m-title")) return;
  $("m-title").innerHTML = `${esc(t.title)} ${statusBadge(t.status)}`;
  $("m-id").textContent = t.id;

  const active = state.runs.find(r => !TERMINAL.includes(r.status));
  $("m-run").disabled = !!active;
  $("m-cancel").disabled = !active;

  let meta = `<div class="sub">${esc(t.scope)} · ${esc(t.agent)} on ${esc(t.runtime)} · ${esc(scheduleLabel(t.schedule))}`;
  if (t.next_run_at) meta += ` · next ${new Date(t.next_run_at).toLocaleString()}`;
  if (t.ack_timeout_seconds) meta += ` · ack ${t.ack_timeout_seconds}s`;
  if (t.timeout_seconds) meta += ` · timeout ${t.timeout_seconds}s`;
  meta += `</div>`;
  const labels = Object.entries(t.labels || {});
  if (labels.length) {
    meta += `<div class="sub">${labels.map(([k, v]) => `<span class="tag">${esc(k)}=${esc(v)}</span>`).join(" ")}</div>`;
  }
  if (t.instructions) meta += `<label>Instructions</label><pre>${esc(t.instructions)}</pre>`;
  const r = selectedRun();
  if (r && r.result) meta += `<label>Result</label><pre>${esc(r.result)}</pre>`;
  if (r && r.error) meta += `<label>Error</label><pre>${esc(r.error)}</pre>`;
  $("m-meta").innerHTML = meta;

  $("m-runs").innerHTML = state.runs.length ? state.runs.map(r => {
    const secs = r.ended_at
      ? Math.round((new Date(r.ended_at) - new Date(r.started_at)) / 1000) + "s"
      : since(r.started_at);
    return `<button class="${r.id === state.run ? "on" : ""}" data-run="${esc(r.id)}">
      ${statusBadge(r.status)} attempt ${r.attempt}
      <span class="sub">${esc(r.trigger)} · ${new Date(r.started_at).toLocaleTimeString()} · ${secs}</span>
    </button>`;
  }).join("") : `<div class="sub">Not run yet.</div>`;

  for (const b of $("m-runs").querySelectorAll("button")) {
    b.onclick = () => { state.run = b.dataset.run; renderModal(); loadJournal(); retimeTerminal(); };
  }
}

export async function loadJournal() {
  if (!$("m-journal")) return;
  const path = state.run
    ? `/api/runs/${state.run}/entries?limit=200`
    : `/api/tasks/${state.open}/entries?limit=200`;
  try {
    const entries = (await api(path)).entries;
    $("m-journal").innerHTML = entries.length
      ? entries.slice().reverse().map(e => `
        <div class="entry">
          <div class="when">${esc(new Date(e.at).toLocaleTimeString())} · <span class="who">${esc(e.source)}</span> · ${esc(e.kind)}</div>
          <div>${esc(e.message)}</div>
        </div>`).join("")
      : `<div class="entry sub">No entries yet.</div>`;
  } catch (e) {
    $("m-journal").innerHTML = `<div class="err">${esc(e.message)}</div>`;
  }
}
