//! The dashboard: the same facts the other pages already hold, read as one
//! page. Nothing here is fetched specially -- the KPIs and the by-scope table
//! are built from `state.tasks` and `state.scopes`, which boot() and the
//! socket already keep current.
//!
//! The inbox lives inside this view, not beside it: a task the agent cannot
//! finish alone is a dashboard fact before it is anything else. `blocked` is a
//! real, first-class status -- "the agent needs a human before it can go on"
//! (`task.rs`) -- set the only way a status ever is, by the agent calling
//! `factory task report`. It sits beside failures and cancellations, newest
//! first. What Factory does not record is the *question*: only that an agent
//! is waiting, not what it asked. The closest thing to an answer is the
//! journal entry the agent wrote when it blocked, if it wrote one, and that is
//! what is shown -- never an invented prompt.

import { $, esc, api, state, since } from "./core.js";
import { inScope, scopeLabel } from "./scopes.js";
import { openTask } from "./tasks.js";
import { openCreate } from "./task-form.js";

const INBOX_LIMIT = 10;

export async function loadDashboard() {
  renderDashboard();
}

export function renderDashboard() {
  const el = $("dash");
  if (!el) return;
  // The rail decides how much of the instance this reads as. Every figure
  // below is a count of what is selected, so a scope's dashboard and the whole
  // instance's are the same page asked a narrower question.
  const tasks = [...state.tasks.values()].filter((t) => inScope(t.scope));
  const scopes = state.scopes.filter((s) => inScope(s.name));

  if (!scopes.length) {
    el.innerHTML = state.scope
      ? `<div class="empty">Nothing in ${esc(scopeLabel())}.</div>`
      : `<div class="empty">This instance declares no scopes yet. Add one to
      <code>.factory/config.yaml</code> and the dashboard, the activity log and the site plan all
      have something to draw -- right now there is nothing to show but this sentence.</div>`;
    return;
  }

  el.innerHTML = `
    <div class="kpis">${kpis(tasks, scopes).join("")}</div>
    <div class="drow">
      <section class="dcard">
        <h3>By scope<span class="r">${scopes.length} scope${scopes.length === 1 ? "" : "s"}</span></h3>
        ${byScopeTable(tasks, scopes)}
      </section>
      <section class="dcard">
        <h3>Inbox<span class="r">nothing here is something an agent may decide for itself</span></h3>
        <div id="dash-inbox"></div>
      </section>
    </div>`;

  renderInbox(tasks);
}

function kpi(k, v, sub, unit) {
  return `<div class="kpi"><span class="k">${esc(k)}</span>
    <span class="v">${esc(v)}${unit ? `<small>${esc(unit)}</small>` : ""}</span>
    <span class="d">${esc(sub)}</span></div>`;
}

function kpis(tasks, scopes) {
  const now = Date.now();
  const day = 24 * 60 * 60 * 1000;
  const running = tasks.filter((t) => t.status === "running" || t.status === "dispatching");
  const blocked = tasks.filter((t) => t.status === "blocked");
  const pending = tasks.filter((t) => t.status === "pending");
  const finishedRecently = tasks.filter(
    (t) => ["done", "failed", "cancelled"].includes(t.status) && now - new Date(t.updated_at).getTime() < day
  );
  const failedRecently = finishedRecently.filter((t) => t.status === "failed" || t.status === "cancelled");
  const rate = finishedRecently.length ? Math.round((failedRecently.length / finishedRecently.length) * 100) : null;
  const agentCount = scopes.reduce((n, s) => n + s.agents.length, 0);
  const standingUp = scopes.reduce(
    (n, s) => n + s.agents.filter((a) => a.state === "ready" || a.state === "starting").length,
    0
  );

  return [
    kpi("In flight", running.length + blocked.length, `${running.length} running · ${blocked.length} blocked`),
    kpi("Queued", pending.length, "created, not yet run"),
    kpi("Finished, 24h", finishedRecently.length, `${failedRecently.length} failed or cancelled`),
    kpi("Failure rate, 24h", rate === null ? "—" : rate, rate === null ? "nothing finished yet" : "failed or cancelled, of finished", rate === null ? "" : "%"),
    kpi("Scopes · agents", `${scopes.length} · ${agentCount}`, `${standingUp} standing agent${standingUp === 1 ? "" : "s"} up`),
  ];
}

function byScopeTable(tasks, scopes) {
  if (!scopes.length) return `<div class="empty">No scopes declared.</div>`;
  const rows = scopes.map((s) => {
    const ts = tasks.filter((t) => t.scope === s.name);
    const active = ts.filter((t) => t.status === "running" || t.status === "dispatching" || t.status === "blocked").length;
    const failed = ts.filter((t) => t.status === "failed").length;
    return `<tr>
      <td><div class="title">${esc(s.name)}</div><div class="sub">${esc(s.path)}</div></td>
      <td class="sub">${s.agents.length}</td>
      <td class="sub">${active}</td>
      <td class="sub">${ts.length}</td>
      <td class="sub">${failed}</td>
    </tr>`;
  }).join("");
  return `<table>
    <thead><tr><th>Scope</th><th>Agents</th><th>Active</th><th>Tasks</th><th>Failed</th></tr></thead>
    <tbody>${rows}</tbody>
  </table>`;
}

/// Every reason an item is in front of a person, in one list: the daemon has
/// no single "needs attention" query, so this reads the same `state.tasks`
/// every other view already has and sorts what it finds by recency.
function inboxItems(tasks) {
  const out = [];
  for (const t of tasks) {
    if (t.status === "blocked") out.push({ t, kind: "blocked", when: t.updated_at, colour: "wait" });
    else if (t.status === "failed") out.push({ t, kind: "failed", when: t.updated_at, colour: "fault" });
    else if (t.status === "cancelled") out.push({ t, kind: "cancelled", when: t.updated_at, colour: "idle" });
    if (t.schedule && t.next_run_at && new Date(t.next_run_at).getTime() < Date.now()) {
      out.push({ t, kind: "overdue", when: t.next_run_at, colour: "wait" });
    }
  }
  out.sort((a, b) => b.when.localeCompare(a.when));
  return out;
}

function renderInbox(tasks) {
  const host = $("dash-inbox");
  if (!host) return;
  const items = inboxItems(tasks).slice(0, INBOX_LIMIT);
  if (!items.length) {
    host.innerHTML = `<div class="empty">Nothing waiting on a person right now.</div>`;
    return;
  }
  host.innerHTML = items.map((it, i) => {
    const label = {
      blocked: "needs a human",
      failed: "failed",
      cancelled: "cancelled",
      overdue: "schedule overdue",
    }[it.kind];
    return `<div class="inbox-item" data-task="${esc(it.t.id)}" data-i="${i}">
      <span class="it-dot" style="background:var(--${it.colour})"></span>
      <span class="it-t"><b>${esc(it.t.title)}</b> — ${esc(label)}
        <span class="sub" data-reason="${esc(it.t.id)}-${i}">${
          it.kind === "blocked" ? "checking what it needs…" : esc(it.t.scope)
        }</span>
      </span>
      <span class="it-age">${since(it.when)}</span>
    </div>`;
  }).join("");

  for (const row of host.querySelectorAll(".inbox-item")) {
    row.onclick = () => openTask(row.dataset.task);
  }

  // Progressive: a blocked task's reason, if the agent sent one, lives in its
  // journal. Fetched after the list paints so the dashboard is not waiting on
  // N requests before it shows anything.
  items.forEach((it, i) => {
    if (it.kind !== "blocked") return;
    api(`/api/tasks/${it.t.id}/entries?limit=20`)
      .then((data) => {
        const entries = data.entries || [];
        const reason = entries.slice().reverse().find((e) => e.kind === "blocked" && e.source === "agent");
        const el = host.querySelector(`[data-reason="${it.t.id}-${i}"]`);
        if (el) el.textContent = reason ? reason.message : "blocked — what it needs is not recorded";
      })
      .catch(() => {});
  });
}

export function wireDashboard() {
  const btn = $("newTaskFromDash");
  if (btn) btn.onclick = () => openCreate();
}
