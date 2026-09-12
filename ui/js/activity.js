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
import { inScope } from "./scopes.js";

const LIMIT = 500;
let log = [];
let kind = "all";
let q = "";
/// When the socket actually opened, not when this view first happened to be
/// shown. app.js's boot() connects the socket once, before anyone can have
/// clicked anything, so an event can already be sitting in `log` by the time
/// a person opens this tab -- stamping "now" at that point would claim a
/// narrower window than what was really captured, which is the same kind of
/// overclaiming the banner exists to rule out.
let watchSince = null;

/// Called from app.js the moment the socket opens. Idempotent: a reconnect
/// re-opens the same socket, but the watch itself started at the first open,
/// not the latest one.
export function markWatching() {
  if (!watchSince) watchSince = new Date();
  updateNote();
}

function updateNote() {
  const el = $("activity-note");
  if (!el) return;
  el.textContent = watchSince
    ? `watching live since ${watchSince.toLocaleTimeString()} — nothing earlier is shown`
    : "connecting…";
}

export function initActivity() {
  updateNote();
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
  // Kept separate from `describe`: the scope a row belongs to is read off
  // the raw event, not the rendered summary, so it has to be worked out here
  // regardless of which fields `describe` chose to put in `label`/`detail`.
  row.scope = scopeOf(ev);
  log.unshift(row);
  if (log.length > LIMIT) log.length = LIMIT;
  if (state.tab === "activity") renderActivity();
}

/// The scope a task-keyed event happened in, read off `state.tasks` -- which
/// still has it, for `task_deleted`, because this runs before `app.js`
/// removes the entry. `undefined` for a task the snapshot never covered (a
/// very old event, or one that raced the initial load), or for an event with
/// no scope of its own (`daemon_started`); `renderActivity`'s filter treats
/// either as nothing to narrow, the way `inScope` fails open elsewhere for a
/// selection it cannot resolve yet.
function scopeOf(ev) {
  switch (ev.type) {
    case "task_created":
    case "task_updated":
      return ev.task.scope;
    case "task_deleted":
    case "task_entry":
      return state.tasks.get(ev.id)?.scope;
    case "run_started":
    case "run_updated":
      return state.tasks.get(ev.run.task_id)?.scope;
    case "agent_updated":
      return ev.agent.scope;
    case "agent_removed": {
      // `<scope>/<name>`, split on the *last* `/` -- a scope's own identity
      // can contain one now that it is a path, so only the agent's own name,
      // after it, is guaranteed to have none.
      const cut = ev.id.lastIndexOf("/");
      return cut < 0 ? undefined : ev.id.slice(0, cut);
    }
    default:
      return undefined;
  }
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

export function renderActivity() {
  const tbody = $("activity-rows");
  const empty = $("activity-empty");
  if (!tbody) return;
  const rows = log.filter((r) => {
    if (kind !== "all" && r.kind !== kind) return false;
    // Same narrowing every other view applies -- `tasks.js`, `agents.js`,
    // `dashboard.js`, `occupancy.js`, `site.js`. `r.scope == null` is an
    // event with no scope of its own (`daemon_started`) or one this page
    // never learned the scope of; either way there is nothing to narrow.
    if (r.scope != null && !inScope(r.scope)) return false;
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
