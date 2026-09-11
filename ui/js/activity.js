//! Activity: a filterable window over the events this page has seen since it
//! opened.
//!
//! The daemon answers "what is true now" (`/api/tasks`, `/api/agents`) and
//! "what happened on this task or run" (its entries), but not "what happened,
//! in order, across everything" -- there is a live `/ws` stream and no queryable
//! history behind it yet. So this is a tail, not an archive, and the banner
//! says so: a log with a silent hole at the bottom reads as complete when it
//! is not, which is worse than admitting the hole.

import { $, esc, state } from "./core.js";

const LIMIT = 500;
let log = [];
let kind = "all";
let q = "";

export function initActivity() {
  const opened = new Date();
  $("activity-note").textContent = `watching live since ${opened.toLocaleTimeString()} — nothing earlier is shown`;
  for (const b of $("activity-kind").querySelectorAll("button")) {
    b.onclick = () => {
      for (const o of $("activity-kind").querySelectorAll("button")) o.classList.remove("on");
      b.classList.add("on");
      kind = b.dataset.k;
      renderActivity();
    };
  }
  $("activity-q").oninput = (e) => {
    q = e.target.value.trim().toLowerCase();
    renderActivity();
  };
  renderActivity();
}

/// Called from app.js for every event the socket delivers, whether or not
/// this view is the one on screen -- the tail has to keep filling while
/// someone is looking at the site plan, or switching here would show a gap
/// starting from just now instead of from when the page opened.
export function recordEvent(ev) {
  const row = describe(ev);
  if (!row) return;
  log.unshift(row);
  if (log.length > LIMIT) log.length = LIMIT;
  if (state.tab === "activity") renderActivity();
}

function describe(ev) {
  const at = new Date();
  switch (ev.type) {
    case "task_created":
      return { at, kind: "task_", label: "task created", detail: `${ev.task.scope} · ${ev.task.title}` };
    case "task_updated":
      return { at, kind: "task_", label: `task → ${ev.task.status}`, detail: `${ev.task.scope} · ${ev.task.title}` };
    case "task_deleted":
      return { at, kind: "task_", label: "task deleted", detail: ev.id };
    case "task_entry":
      return {
        at,
        kind: "task_",
        label: "journal entry",
        detail: `${ev.entry.source} · ${ev.entry.kind}: ${ev.entry.message}`,
      };
    case "run_started":
      return { at, kind: "run_", label: "run started", detail: `task ${ev.run.task_id} · attempt ${ev.run.attempt} · ${ev.run.trigger}` };
    case "run_updated":
      return { at, kind: "run_", label: `run → ${ev.run.status}`, detail: `task ${ev.run.task_id} · attempt ${ev.run.attempt}` };
    case "agent_updated":
      return { at, kind: "agent_", label: "agent updated", detail: `${ev.agent.scope}/${ev.agent.name} · ${ev.agent.state}` };
    case "agent_removed":
      return { at, kind: "agent_", label: "agent removed", detail: ev.id };
    case "daemon_started":
      return { at, kind: "all", label: "daemon started", detail: ev.instance };
    default:
      return null;
  }
}

function renderActivity() {
  const tbody = $("activity-rows");
  const empty = $("activity-empty");
  if (!tbody) return;
  const rows = log.filter((r) => {
    if (kind !== "all" && r.kind !== kind) return false;
    if (!q) return true;
    return (r.label + " " + r.detail).toLowerCase().includes(q);
  });
  $("activity-count").textContent = log.length ? `${rows.length} of ${log.length} seen` : "";
  empty.hidden = log.length > 0;
  tbody.innerHTML = rows.map((r) => `
    <tr>
      <td class="sub">${esc(r.at.toLocaleTimeString())}</td>
      <td><span class="badge">${esc(r.label)}</span></td>
      <td class="sub">${esc(r.detail)}</td>
    </tr>`).join("");
}
