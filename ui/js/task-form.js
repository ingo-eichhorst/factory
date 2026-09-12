//! Creating and editing a task. One set of fields, shared by both, so a task
//! made in the browser can say everything a task made on the command line can.

import { $, esc, api, state } from "./core.js";
import { scrim, closeModal } from "./modal.js";
// The form and the task modal open each other: edit comes from the modal, and
// saving goes back to it. A cycle ES modules handle, because nothing here runs
// until a button is pressed.
import { openTask, renderTasks } from "./tasks.js";

export function openCreate(prefill) {
  closeModal();
  prefill = prefill || {};
  scrim(`
    <header><div><h2>New task</h2></div><button class="x" id="c-close">&times;</button></header>
    <div class="body">
      ${taskFields(prefill)}
      <div class="err" id="c-err"></div>
      <div class="row-btns" style="margin-top:16px">
        <button class="btn primary" id="c-create">Create</button>
        <button class="btn" id="c-createrun">Create and run</button>
      </div>
    </div>`);
  $("c-close").onclick = closeModal;
  $("c-create").onclick = () => create(false);
  $("c-createrun").onclick = () => create(true);
  wireScopeAgent();
  $("c-title").focus();
}

/// Editing is the same fields, sent as a patch. A schedule set here has to be
/// rescheduled by the daemon, which is why this is a PATCH rather than a write
/// straight into the store.
export function openEdit(task) {
  closeModal();
  scrim(`
    <header><div><h2>Edit task</h2><code class="id">${esc(task.id)}</code></div>
      <button class="x" id="c-close">&times;</button></header>
    <div class="body">
      ${taskFields({
        title: task.title,
        instructions: task.instructions,
        scope: task.scope,
        agent: task.agent,
        scheduleText: scheduleText(task.schedule),
        ack_timeout_seconds: task.ack_timeout_seconds,
        timeout_seconds: task.timeout_seconds,
        labelText: Object.entries(task.labels || {}).map(([k, v]) => `${k}=${v}`).join("\n"),
        worktree: task.worktree,
        editing: true,
      })}
      <div class="err" id="c-err"></div>
      <div class="row-btns" style="margin-top:16px">
        <button class="btn primary" id="c-save">Save</button>
        <button class="btn" id="c-cancel">Cancel</button>
      </div>
    </div>`);
  $("c-close").onclick = closeModal;
  $("c-cancel").onclick = () => openTask(task.id);
  $("c-save").onclick = async () => {
    $("c-err").textContent = "";
    try {
      const f = readTaskFields();
      await api(`/api/tasks/${task.id}`, {
        method: "PATCH",
        body: JSON.stringify({
          ...f,
          // `null` means "leave alone" on the wire, so going back to the
          // instance default and dropping a schedule say so explicitly.
          clear_schedule: f.schedule === null,
          clear_ack_timeout: f.ack_timeout_seconds === null,
          clear_timeout: f.timeout_seconds === null,
        }),
      });
      openTask(task.id);
    } catch (e) { $("c-err").textContent = e.message; }
  };
  wireScopeAgent();
}

/// What a task in this scope can be given to: the agents the scope declares, by
/// name, and then any adapter that is not one of them. A scope's `assistant` and
/// its `scratch` are different agents even when both are pi -- naming the
/// harness would not tell them apart.
export function agentOptions(scopeName, selected) {
  const scope = state.scopes.find(s => s.name === scopeName);
  const out = [`<option value="">(scope default)</option>`];
  const named = new Set();
  if (scope) {
    for (const a of scope.agents) {
      named.add(a.name);
      const hint = a.adapter !== a.name ? ` — ${a.adapter}` : "";
      const life = a.lifetime !== "task" ? ` (${a.lifetime})` : "";
      out.push(`<option value="${esc(a.name)}" ${a.name === selected ? "selected" : ""}>${esc(a.name)}${esc(hint)}${esc(life)}</option>`);
    }
  }
  const rest = state.adapters.filter(a => !named.has(a));
  if (rest.length) {
    out.push(`<optgroup label="any adapter">${rest.map(a =>
      `<option value="${esc(a)}" ${a === selected ? "selected" : ""}>${esc(a)}</option>`).join("")}</optgroup>`);
  }
  return out.join("");
}

export function scopeOptions(selected) {
  return `<option value="">(first scope)</option>` + state.scopeNames.map(s =>
    `<option value="${esc(s)}" ${s === selected ? "selected" : ""}>${esc(s)}</option>`).join("");
}

/// Whether a scope, as the agents page last reported it, can host a worktree
/// -- and if not, why. An unknown scope (not loaded yet, or named freehand)
/// is assumed capable rather than blocking the form on a question it cannot
/// yet answer.
function worktreeCapability(scopeName) {
  const scope = state.scopes.find(s => s.name === scopeName);
  if (!scope) return { capable: true, reason: null };
  return { capable: scope.worktree_capable !== false, reason: scope.worktree_reason || null };
}

/// The line under the checkbox: what it does, why it is disabled, or that
/// editing cannot move a task that is already running somewhere.
function worktreeHint(capable, reason, editing) {
  if (editing) return "Set when the task was created; its runs keep using it.";
  if (!capable) return `Disabled -- ${esc(reason || "this scope cannot have one")}.`;
  return "A fresh git worktree and branch, made before each run. Nothing merges it for you.";
}

/// The fields a task carries beyond its title. Shared by create and edit so the
/// two cannot drift into offering different things -- except the worktree
/// checkbox, which edit shows but cannot move: a task's worktree setting is
/// fixed at creation, the same as its id.
export function taskFields(v) {
  v = v || {};
  const editing = v.editing === true;
  const scopeName = v.scope || state.scopeNames[0];
  const { capable, reason } = worktreeCapability(scopeName);
  const checked = capable && v.worktree !== false;
  const disabled = editing || !capable;
  return `
    <label for="c-title">Title</label>
    <input id="c-title" placeholder="What should happen" value="${esc(v.title || "")}">
    <label for="c-instructions">Instructions</label>
    <textarea id="c-instructions" placeholder="What the agent is being asked to do">${esc(v.instructions || "")}</textarea>
    <div class="grid2">
      <div><label for="c-scope">Scope</label>
        <select id="c-scope">${scopeOptions(v.scope)}</select></div>
      <div><label for="c-agent">Agent</label>
        <select id="c-agent">${agentOptions(v.scope || state.scopeNames[0], v.agent)}</select></div>
    </div>
    <label for="c-worktree">Worktree</label>
    <div class="checkrow">
      <input type="checkbox" id="c-worktree" data-editing="${editing ? "1" : "0"}"
        ${checked ? "checked" : ""} ${disabled ? "disabled" : ""}>
      <span>Work in its own git worktree</span>
    </div>
    <div class="sub" id="c-worktree-sub">${worktreeHint(capable, reason, editing)}</div>
    <label for="c-schedule">Schedule <span class="sub" style="text-transform:none">(blank = manual)</span></label>
    <input id="c-schedule" placeholder="every 5m  ·  0 9 * * 1-5" value="${esc(v.scheduleText || "")}">
    <div class="grid2">
      <div><label for="c-ack">Acknowledge within <span class="sub" style="text-transform:none">seconds</span></label>
        <input id="c-ack" type="number" min="1" placeholder="instance default" value="${v.ack_timeout_seconds ?? ""}"></div>
      <div><label for="c-timeout">Run timeout <span class="sub" style="text-transform:none">seconds</span></label>
        <input id="c-timeout" type="number" min="1" placeholder="instance default" value="${v.timeout_seconds ?? ""}"></div>
    </div>
    <label for="c-labels">Labels <span class="sub" style="text-transform:none">key=value, one per line</span></label>
    <textarea id="c-labels" style="min-height:52px" placeholder="area=infra">${esc(v.labelText || "")}</textarea>`;
}

/// Changing the scope changes which agents exist, so the second select is
/// rebuilt rather than left showing an agent the new scope has never heard of.
/// It can also change whether a worktree is possible at all, so the checkbox
/// is re-read the same way.
export function wireScopeAgent() {
  const scope = $("c-scope");
  if (!scope) return;
  scope.onchange = () => {
    $("c-agent").innerHTML = agentOptions(scope.value || state.scopeNames[0], null);
    refreshWorktreeField(scope.value || state.scopeNames[0]);
  };
}

function refreshWorktreeField(scopeName) {
  const box = $("c-worktree");
  if (!box) return;
  const editing = box.dataset.editing === "1";
  const { capable, reason } = worktreeCapability(scopeName);
  box.disabled = editing || !capable;
  if (!capable) box.checked = false;
  else if (!editing) box.checked = true;
  const sub = $("c-worktree-sub");
  if (sub) sub.innerHTML = worktreeHint(capable, reason, editing);
}

export function readTaskFields() {
  const labels = {};
  for (const line of $("c-labels").value.split("\n")) {
    const s = line.trim();
    if (!s) continue;
    const i = s.indexOf("=");
    if (i < 1) throw new Error(`labels look like key=value, not "${s}"`);
    labels[s.slice(0, i).trim()] = s.slice(i + 1).trim();
  }
  const num = (id) => {
    const v = $(id).value.trim();
    return v ? parseInt(v, 10) : null;
  };
  return {
    title: $("c-title").value.trim(),
    instructions: $("c-instructions").value,
    scope: $("c-scope").value || null,
    agent: $("c-agent").value || null,
    schedule: parseSchedule($("c-schedule").value),
    ack_timeout_seconds: num("c-ack"),
    timeout_seconds: num("c-timeout"),
    labels,
  };
}

export function scheduleText(s) {
  if (!s) return "";
  if (s.cron) return s.cron;
  if (s.every) return `every ${s.every.seconds}s`;
  return "";
}

export function parseSchedule(text) {
  const v = text.trim();
  if (!v) return null;
  const m = v.match(/^every\s+(\d+)\s*(s|sec|secs|seconds|m|min|mins|minutes|h|hrs?|hours?)?$/i);
  if (m) {
    const n = parseInt(m[1], 10);
    const u = (m[2] || "s").toLowerCase();
    const mult = u.startsWith("h") ? 3600 : (u.startsWith("m") ? 60 : 1);
    return { every: { seconds: n * mult } };
  }
  return { cron: v.replace(/^cron\s+/i, "") };
}

export async function create(andRun) {
  $("c-err").textContent = "";
  try {
    const box = $("c-worktree");
    const data = await api("/api/tasks", {
      method: "POST",
      body: JSON.stringify({
        ...readTaskFields(),
        // Sent explicitly either way, not left absent: absent would mean
        // "on" to the daemon too, which is wrong the moment the scope itself
        // cannot have one and the box is unchecked because of it.
        worktree: box ? box.checked : true,
      }),
    });
    const task = data.task;
    state.tasks.set(task.id, task);
    renderTasks();
    if (andRun) await api(`/api/tasks/${task.id}/run`, { method: "POST" });
    openTask(task.id);
  } catch (e) {
    $("c-err").textContent = e.message;
  }
}
