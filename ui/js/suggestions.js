//! The L5 Improvement Suggestions tab (`#275`): what agents flagged as
//! friction or an improvement idea while they worked, newest first,
//! filterable, folded by target with a count and summed cost -- the
//! prioritisation signal the issue asks for. `suggestions-model.js` holds
//! every pure shaping function this file draws from; this file only
//! fetches, draws and wires clicks, the same split `quality.js` keeps.
//!
//! Nothing here acts on its own. A suggestion becomes work only when a
//! person presses "Create improvement task" (or `factory suggestion
//! task`) -- the one write besides dismiss/done/ask, and every one of
//! those asks inline before it acts, never a `confirm()` dialog.
//!
//! No poll: the report is read fresh on every load, like Quality/Goals/
//! Policy, and `app.js` refetches on a `suggestion_filed`-shaped event
//! (a task entry of kind `suggestion`) once that wiring exists there.

import { $, api, esc, state } from "./core.js";
import {
  canAsk,
  canDismiss,
  canDone,
  dismissBody,
  formatCost,
  formatTokens,
  groupSummaryLine,
  groupSuggestions,
  kindLabel,
  askBody,
  shortId,
  stateLabel,
  suggestionRunHref,
  suggestionTaskHref,
  taskableIds,
  taskBody,
  validDismissReason,
  validQuestion,
  visibleGroups,
} from "./suggestions-model.js";

let asked = 0;
let report = null;
let failure = null;

/// Ephemeral UI-only state, the same reasoning `quality.js`'s own
/// `remediation` map gives: a person's place in a multi-step action
/// (confirm, then act) survives a redraw, but never a fetch error.
let kindFilter = "";
let stateFilter = "";
let query = "";
/// `"dismiss:<id>"` or `"ask:<id>"` while that row's inline form is open;
/// `{phase:"pending"|"error", message?}` while a request for it is out.
const pending = new Map();
let openForm = null;

export async function loadSuggestions() {
  const mine = ++asked;
  const params = new URLSearchParams();
  if (state.scope !== null) params.set("scope", state.scope);
  if (kindFilter) params.set("kind", kindFilter);
  if (stateFilter) params.set("state", stateFilter);
  try {
    const answer = await api(`/api/suggestions${params.toString() ? `?${params}` : ""}`);
    if (mine !== asked) return;
    report = answer.report;
    failure = null;
  } catch (e) {
    if (mine !== asked) return;
    report = null;
    failure = e.message;
  }
  renderSuggestions();
}

export function reloadSuggestions() {
  loadSuggestions();
}

export function wireSuggestions() {
  const refresh = $("suggestions-refresh");
  if (refresh) refresh.onclick = () => loadSuggestions();
  const kind = $("suggestions-kind");
  if (kind) kind.onchange = () => { kindFilter = kind.value; loadSuggestions(); };
  const st = $("suggestions-state");
  if (st) st.onchange = () => { stateFilter = st.value; loadSuggestions(); };
  const q = $("suggestions-query");
  if (q) q.oninput = () => { query = q.value; renderSuggestions(); };
}

function statusChip(s) {
  return `<span class="badge s-${esc(s)}">${esc(stateLabel(s))}</span>`;
}

export function renderSuggestions() {
  const failed = $("suggestions-error");
  if (failed) {
    failed.textContent = failure || "";
    failed.hidden = !failure;
  }
  const empty = $("suggestions-empty");
  const groupsEl = $("suggestions-groups");
  const count = $("suggestions-count");
  const groups = visibleGroups(report, query);
  if (count) count.textContent = report ? `${report.suggestions.length} suggestion${report.suggestions.length === 1 ? "" : "s"}` : "";
  if (!report || groups.length === 0) {
    if (empty) empty.hidden = !report && !!failure;
    if (groupsEl) groupsEl.innerHTML = "";
    return;
  }
  if (empty) empty.hidden = true;
  if (groupsEl) groupsEl.innerHTML = groups.map((g) => groupHtml(g)).join("");
  wireActions(groupsEl);
}

function groupHtml(group) {
  const rows = groupSuggestions(report, group);
  // Only the ids `suggestion_task` would actually accept: one already
  // dismissed or done in the same group must not sink the whole request
  // (`#275` QA) -- the button sends exactly `taskableIds`, never the
  // group's full (possibly multi-scope) `ids`.
  const ids = taskableIds(report, group);
  return `<section class="sug-group">
    <div class="bar sug-group-head">
      <div>
        <div class="title">[${esc(kindLabel(group.kind))}] ${esc(group.target)}</div>
        <span class="sub">${esc(groupSummaryLine(group))}</span>
      </div>
      <span class="sp"></span>
      ${ids.length ? `<button type="button" class="btn primary" data-sug-group-task="${esc(ids.join(","))}">Create improvement task</button>` : ""}
    </div>
    <table class="sug-rows">
      <tbody>${rows.map((s) => rowHtml(s)).join("")}</tbody>
    </table>
  </section>`;
}

function rowHtml(s) {
  const key = (action) => `${action}:${s.id}`;
  const entry = pending.get(key("dismiss")) || pending.get(key("ask")) || pending.get(key("task"));
  const taskLink = s.improvement_task_id
    ? `<a href="${esc(suggestionTaskHref(s.scope, s.improvement_task_id))}">task →</a>`
    : "";
  const runLink = `<a href="${esc(suggestionRunHref(s.scope, s.task_id, s.run_id))}">${esc(s.scope)}/${esc(s.agent)} →</a>`;
  const cost = formatCost(s.usage && s.usage.cost_usd);
  const tokens = s.wasted_tokens ? `~${esc(formatTokens(s.wasted_tokens))} tokens` : "";
  return `<tr class="sug-row" data-sug-id="${esc(s.id)}">
    <td>${statusChip(s.state)}</td>
    <td>
      <div class="title">${esc(s.summary)}</div>
      ${s.detail ? `<div class="sub">${esc(s.detail)}</div>` : ""}
      <div class="sub">${runLink} · <code class="id">${esc(shortId(s.id))}</code>${taskLink ? ` · ${taskLink}` : ""}</div>
      ${actionAreaHtml(s, entry)}
    </td>
    <td class="sub">${[tokens, cost].filter(Boolean).join(" · ") || "—"}</td>
    <td><div class="sug-row-actions">
      ${canDismiss(s) ? `<button type="button" class="btn" data-sug-open-dismiss="${esc(s.id)}">Dismiss</button>` : ""}
      ${canDone(s) ? `<button type="button" class="btn" data-sug-done="${esc(s.id)}">Done</button>` : ""}
      ${canAsk(s) ? `<button type="button" class="btn" data-sug-open-ask="${esc(s.id)}">Ask the agent</button>` : ""}
    </div></td>
  </tr>`;
}

function actionAreaHtml(s, entry) {
  const error = entry && entry.phase === "error" ? `<div class="sub sug-err">${esc(entry.message)}</div>` : "";
  if (s.ask && openForm !== `ask:${s.id}`) {
    return `<div class="sug-ask-answer"><div class="sub">asked: ${esc(s.ask.question)}</div>
      <div class="sub">${s.ask.answer ? `answered: ${esc(s.ask.answer)}` : "waiting on the resumed run…"}</div></div>${error}`;
  }
  if (openForm === `dismiss:${s.id}`) {
    const busy = entry && entry.phase === "pending";
    return `<div class="sug-form" data-sug-dismiss-form="${esc(s.id)}">
      <input type="text" placeholder="Why?" data-sug-reason${busy ? " disabled" : ""}>
      <button type="button" class="btn primary" data-sug-confirm-dismiss="${esc(s.id)}"${busy ? " disabled" : ""}>${busy ? "Dismissing…" : "Dismiss"}</button>
      <button type="button" class="btn" data-sug-cancel-form>Cancel</button>
    </div>${error}`;
  }
  if (openForm === `ask:${s.id}`) {
    const busy = entry && entry.phase === "pending";
    return `<div class="sug-form" data-sug-ask-form="${esc(s.id)}">
      <input type="text" placeholder="Ask a follow-up question…" data-sug-question${busy ? " disabled" : ""}>
      <button type="button" class="btn primary" data-sug-confirm-ask="${esc(s.id)}"${busy ? " disabled" : ""}>${busy ? "Asking…" : "Ask"}</button>
      <button type="button" class="btn" data-sug-cancel-form>Cancel</button>
    </div>${error}`;
  }
  return error;
}

function wireActions(root) {
  if (!root) return;
  for (const b of root.querySelectorAll("[data-sug-group-task]")) {
    b.onclick = () => createImprovementTask(b.dataset.sugGroupTask.split(","));
  }
  for (const b of root.querySelectorAll("[data-sug-open-dismiss]")) {
    b.onclick = () => { openForm = `dismiss:${b.dataset.sugOpenDismiss}`; renderSuggestions(); };
  }
  for (const b of root.querySelectorAll("[data-sug-open-ask]")) {
    b.onclick = () => { openForm = `ask:${b.dataset.sugOpenAsk}`; renderSuggestions(); };
  }
  for (const b of root.querySelectorAll("[data-sug-cancel-form]")) {
    b.onclick = () => { openForm = null; renderSuggestions(); };
  }
  for (const b of root.querySelectorAll("[data-sug-done]")) {
    b.onclick = () => markDone(b.dataset.sugDone);
  }
  for (const b of root.querySelectorAll("[data-sug-confirm-dismiss]")) {
    b.onclick = () => {
      const id = b.dataset.sugConfirmDismiss;
      const input = root.querySelector(`[data-sug-dismiss-form="${id}"] [data-sug-reason]`);
      confirmDismiss(id, input ? input.value : "");
    };
  }
  for (const b of root.querySelectorAll("[data-sug-confirm-ask]")) {
    b.onclick = () => {
      const id = b.dataset.sugConfirmAsk;
      const input = root.querySelector(`[data-sug-ask-form="${id}"] [data-sug-question]`);
      confirmAsk(id, input ? input.value : "");
    };
  }
}

export async function createImprovementTask(ids) {
  pending.set(`task:${ids[0]}`, { phase: "pending" });
  renderSuggestions();
  try {
    await api("/api/suggestions/task", { method: "POST", body: JSON.stringify(taskBody(ids)) });
    for (const id of ids) pending.delete(`task:${id}`);
    await loadSuggestions();
  } catch (e) {
    pending.set(`task:${ids[0]}`, { phase: "error", message: e.message });
    renderSuggestions();
  }
}

export async function confirmDismiss(id, reason) {
  if (!validDismissReason(reason)) {
    pending.set(`dismiss:${id}`, { phase: "error", message: "give a reason before dismissing it" });
    renderSuggestions();
    return;
  }
  pending.set(`dismiss:${id}`, { phase: "pending" });
  renderSuggestions();
  try {
    await api(`/api/suggestions/${id}/dismiss`, { method: "POST", body: JSON.stringify(dismissBody(reason)) });
    pending.delete(`dismiss:${id}`);
    openForm = null;
    await loadSuggestions();
  } catch (e) {
    pending.set(`dismiss:${id}`, { phase: "error", message: e.message });
    renderSuggestions();
  }
}

export async function markDone(id) {
  pending.set(`done:${id}`, { phase: "pending" });
  renderSuggestions();
  try {
    await api(`/api/suggestions/${id}/done`, { method: "POST", body: "{}" });
    pending.delete(`done:${id}`);
    await loadSuggestions();
  } catch (e) {
    pending.set(`done:${id}`, { phase: "error", message: e.message });
    renderSuggestions();
  }
}

export async function confirmAsk(id, question) {
  if (!validQuestion(question)) {
    pending.set(`ask:${id}`, { phase: "error", message: "ask something before sending it" });
    renderSuggestions();
    return;
  }
  pending.set(`ask:${id}`, { phase: "pending" });
  renderSuggestions();
  try {
    await api(`/api/suggestions/${id}/ask`, { method: "POST", body: JSON.stringify(askBody(question)) });
    pending.delete(`ask:${id}`);
    openForm = null;
    await loadSuggestions();
  } catch (e) {
    pending.set(`ask:${id}`, { phase: "error", message: e.message });
    renderSuggestions();
  }
}
