//! The task list and the task modal: its runs, its journal, and the terminal of
//! whichever run is selected.

import { $, esc, api, state, since, shortSpan, statusBadge, TERMINAL } from "./core.js";
import { inScope, scopeLabel, writeHash } from "./scopes.js";
import { scrim, closeModal, dropModal } from "./modal.js";
import { terminalBlock, wireTerminal, setTerminal } from "./terminal.js";
import { openEdit, scheduleText } from "./task-form.js";
import { scheduleLabel } from "./schedule.js";
import { describeWorkflowOrigin } from "./workflows.js";
import { entryKindLabel, entryTone } from "./operations-model.js";
import { runUsageView, taskUsageLine } from "./usage-model.js";
import { notStartedNote } from "./pending-model.js";

export { scheduleLabel };

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

/// A paused schedule, wherever a scheduled task is drawn (`#106`): the rule
/// is still there and shown, and nothing fires until someone resumes it --
/// a stopped line that reads as a running one is worse than no badge.
export function pausedTag(t) {
  return t.schedule && t.schedule_paused
    ? ` <span class="tag paused" title="schedule paused: nothing fires until it is resumed">paused</span>`
    : "";
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
  // Manual means nothing will start it: there is no queue (`#124`).
  { key: "manual", label: "Manual · not run" },
  { key: "scheduled", label: "Scheduled" },
  { key: "active", label: "In progress" },
  { key: "closed", label: "Closed" },
];

function columnFor(t) {
  if (t.status === "blocked") return "blocked";
  if (t.status === "pending") return t.schedule ? "scheduled" : "manual";
  // `verifying` is still in progress: the agent said done and the daemon is
  // running the steps its control plan requires (`#118`).
  if (t.status === "dispatching" || t.status === "running" || t.status === "verifying") return "active";
  return "closed";
}

/// Only what `/api/tasks` already serves: title, short id, status, scope,
/// agent, the schedule rule or the run count, its estimate when it has one,
/// and how long the newest run has been going.
function taskCard(t) {
  const bits = [];
  bits.push(t.schedule ? scheduleLabel(t.schedule) : (t.runs ? `${t.runs} run${t.runs === 1 ? "" : "s"}` : "no runs yet"));
  if (t.estimate_seconds) bits.push(`est. ${shortSpan(t.estimate_seconds)}`);
  if (!TERMINAL.includes(t.status) && t.status !== "pending" && t.last_run_at) bits.push(since(t.last_run_at));
  const wt = t.worktree ? ` <span class="tag" title="runs in a git worktree of its own">worktree</span>` : "";
  return `
    <div class="kbc" data-id="${esc(t.id)}">
      <div class="kbc-top">${statusBadge(t.status)}<code class="id">${esc(t.id.slice(0, 8))}</code></div>
      <div class="title">${esc(t.title)}</div>
      <div class="sub">${esc(t.scope)} · ${esc(t.agent)}${wt}</div>
      <div class="sub">${esc(bits.join(" · "))}${pausedTag(t)}</div>
    </div>`;
}

function renderBoard(rows) {
  const byCol = new Map(KANBAN_COLUMNS.map(c => [c.key, []]));
  // An item still in intake (`#119`) is not work on the line yet: it has
  // no run and cannot start one, and it has a board of its own. Left in,
  // `columnFor` would file it under Closed.
  for (const t of rows) if (t.status !== "intake") byCol.get(columnFor(t)).push(t);
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
  // The selection is applied here and not by asking the daemon for one scope:
  // the socket sends the whole task list again on connect and after a lag, and
  // replaces `state.tasks` with it. A fetched subset would be flooded away.
  const rows = [...state.tasks.values()]
    .filter(t => inScope(t.scope))
    .sort((a, b) => b.created_at.localeCompare(a.created_at));
  $("noTasks").hidden = rows.length > 0;
  // Empty under a selection is a fact about the scope, not about the instance.
  $("noTasks").textContent = state.scope ? `No tasks in ${scopeLabel()}.` : "Nothing here yet.";
  $("tasks").innerHTML = rows.map(t => `
    <tr class="row" data-id="${esc(t.id)}">
      <td><div class="title">${esc(t.title)}</div>
          <div class="sub">${esc(scheduleLabel(t.schedule))}${pausedTag(t)}</div></td>
      <td>${statusBadge(t.status)}</td>
      <td class="sub">${t.runs || 0}</td>
      <td class="sub">${esc(t.scope)}</td>
      <td class="sub">${esc(t.agent)}${t.worktree ? ` <span class="tag" title="runs in a git worktree of its own">worktree</span>` : ""}</td>
    </tr>`).join("");
  for (const tr of $("tasks").querySelectorAll("tr.row")) {
    tr.onclick = () => openTask(tr.dataset.id);
  }

  renderBoard(rows);
  // Six empty columns say less than one sentence: with nothing to show, the
  // board steps aside for the same "Nothing here yet." the table already has.
  if (tasksView === "board") $("kanban").hidden = rows.length === 0;
}

/// A task is a place, and it opens over whichever view you were on -- the list,
/// the dashboard's history, the roster, the occupancy chart, a hall on the site
/// plan. So the URL is written here rather than at each of those call sites, and
/// pushed: Back is how you close it.
export async function openTask(id, runId) {
  dropModal();
  state.open = id;
  state.run = runId || null;
  writeHash();
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
  // `loadRuns` picks the newest run when the link did not name one, so the URL
  // only now knows what is on screen. A correction, not a move: replace.
  writeHash(true);
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
  await loadTaskUsage();
}

/// The open task's usage summed over every run (#117) -- read from the
/// daemon rather than re-added here, so the modal and `factory task show`
/// can never disagree about what counts. A daemon without the endpoint
/// leaves it blank.
export async function loadTaskUsage() {
  const open = state.open;
  try {
    const usage = (await api(`/api/tasks/${open}/usage`)).usage;
    if (state.open === open) state.taskUsage = usage;
  } catch { state.taskUsage = null; }
}

/// A run's usage block: tokens, cost and what they rest on, or why there
/// is none. See `usage-model.js`.
function usageHtml(usage) {
  const v = runUsageView(usage);
  let h = `<label>Usage</label><div class="usage usage-${v.tone}"><div${v.tone === "known" ? "" : ` class="sub"`}>${esc(v.headline)}</div>`;
  for (const line of v.lines) h += `<div class="sub">${esc(line)}</div>`;
  for (const note of v.notes) h += `<div class="sub warn">${esc(note)}</div>`;
  return h + `</div>`;
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

  let meta = `<div class="sub">${esc(t.scope)} · ${esc(t.agent)} on ${esc(t.runtime)} · ${esc(scheduleLabel(t.schedule))}${pausedTag(t)}`;
  if (t.next_run_at && !t.schedule_paused) meta += ` · next ${new Date(t.next_run_at).toLocaleString()}`;
  if (t.estimate_seconds) meta += ` · estimate ${shortSpan(t.estimate_seconds)}`;
  if (t.ack_timeout_seconds) meta += ` · ack ${t.ack_timeout_seconds}s`;
  if (t.timeout_seconds) meta += ` · timeout ${t.timeout_seconds}s`;
  if (t.worktree) meta += ` · own worktree`;
  if (t.knowledge_hints) meta += ` · knowledge hints`;
  meta += `</div>`;
  if (t.workflow_origin) {
    // R9: this task's provenance, when a workflow spawned it -- linking
    // back to the run that did, falling back to bare ids if that
    // definition is not one this view has ever cached (deleted, or the
    // Workflows tab simply has not been opened this session).
    const origin = describeWorkflowOrigin(t.workflow_origin);
    meta += `<div class="sub">Workflow: <a href="${esc(origin.href)}">${esc(origin.label)}</a></div>`;
  }
  const labels = Object.entries(t.labels || {});
  if (labels.length) {
    meta += `<div class="sub">${labels.map(([k, v]) => `<span class="tag">${esc(k)}=${esc(v)}</span>`).join(" ")}</div>`;
  }
  const idle = notStartedNote(t);
  if (idle) meta += `<div class="sub">${esc(idle)}</div>`;
  if (t.instructions) meta += `<label>Instructions</label><pre>${esc(t.instructions)}</pre>`;
  const r = selectedRun();
  // The branch and the path this attempt worked in, next to the attach
  // command a person would use to go look at the session itself.
  if (r && r.worktree_branch) {
    meta += `<div class="sub">worktree <code>${esc(r.worktree_branch)}</code>`;
    if (r.worktree_path) meta += ` at <code>${esc(r.worktree_path)}</code>`;
    meta += `</div>`;
  }
  if (r) meta += usageHtml(r.usage);
  const total = state.taskUsage && state.taskUsage.task_id === t.id ? taskUsageLine(state.taskUsage.total) : null;
  if (total) meta += `<div class="sub">${esc(total)}</div>`;
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
    b.onclick = () => {
      state.run = b.dataset.run;
      writeHash();
      renderModal(); loadJournal(); retimeTerminal();
    };
  }
}

/// One journal line. `owner` is a person acting through Factory -- the
/// Operations actions and `--reason` on the CLI (`#106`) -- and is marked
/// apart from the daemon and the agent, since "who did this" is the point
/// of those entries. The message already carries who asked and why; an
/// answer's own text is never in it, and the line says so.
function entryHtml(e) {
  const tone = entryTone(e);
  const note = e.kind === "answer" ? `<div class="sub">What was typed is not recorded -- only that someone answered, and why.</div>` : "";
  return `
        <div class="entry${tone ? ` e-${tone}` : ""}">
          <div class="when">${esc(new Date(e.at).toLocaleTimeString())} · <span class="who" data-source="${esc(e.source)}">${esc(e.source)}</span> · ${esc(entryKindLabel(e.kind))}</div>
          <div>${esc(e.message)}</div>${note}
        </div>`;
}

export async function loadJournal() {
  if (!$("m-journal")) return;
  try {
    let entries;
    if (state.run) {
      // A run's own lines, and the task's lines that belong to no run --
      // a schedule paused or resumed, a slot skipped, a run asked for before
      // it existed. Without the second read those never show once the task
      // has run, since a run is always selected then.
      // `task_only` reads those alone, so a chatty run elsewhere in the
      // task's journal cannot crowd them out of the newest two hundred.
      const [run, task] = await Promise.all([
        api(`/api/runs/${state.run}/entries?limit=200`),
        api(`/api/tasks/${state.open}/entries?limit=200&task_only=true`).catch(() => ({ entries: [] })),
      ]);
      entries = [...run.entries, ...(task.entries || []).filter(e => !e.run_id)]
        .sort((a, b) => Date.parse(a.at) - Date.parse(b.at));
    } else {
      entries = (await api(`/api/tasks/${state.open}/entries?limit=200`)).entries;
    }
    $("m-journal").innerHTML = entries.length
      ? entries.slice().reverse().map(entryHtml).join("")
      : `<div class="entry sub">No entries yet.</div>`;
  } catch (e) {
    $("m-journal").innerHTML = `<div class="err">${esc(e.message)}</div>`;
  }
}
