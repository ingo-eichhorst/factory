//! The roster: every scope, the agents it declares, what each one is doing, and
//! the buttons that start, stop and type at them.

import { $, esc, api, state, since, statusBadge } from "./core.js";
import { inScope, scopeLabel } from "./scopes.js";
import { scrim, closeModal, dropModal } from "./modal.js";
import { terminalBlock, wireTerminal, setTerminal } from "./terminal.js";
import { openTask } from "./tasks.js";
import { openCreate } from "./task-form.js";
// The roster and the wiring open each other: the wiring builds the rail at
// boot, and this is the other place /api/agents is refetched, which is the
// only thing the rail's tree is made of. A cycle ES modules handle, because
// nothing here runs until a page is shown.
import { rebuildRail } from "./app.js";

export async function loadAgents() {
  try {
    const board = await api("/api/agents");
    state.scopes = board.scopes;
    state.roles = board.roles || [];
    if (state.scopes.length) state.adapters = state.scopes[0].available;
    // A scope added or dropped in the config has to reach the rail, not just
    // the roster underneath it.
    rebuildRail();
    renderAgents();
  } catch (e) {
    $("agents").innerHTML = `<div class="err">${esc(e.message)}</div>`;
  }
}


export function agentTags(a) {
  const tags = [];
  // Every role shows, not just the one that used to be the only one worth
  // saying. A role somebody gave says so, because the config says otherwise.
  if (a.role && a.role !== "worker")
    tags.push(`<span class="tag ${a.role === "foreman" ? "foreman" : "role"}">${esc(a.role)}</span>`);
  if (a.assigned_role) tags.push(`<span class="tag">given</span>`);
  if (a.lifetime === "permanent") tags.push(`<span class="tag perm">permanent</span>`);
  else if (a.lifetime === "temporary") tags.push(`<span class="tag">temporary</span>`);
  if (a.is_default) tags.push(`<span class="tag">default</span>`);
  if (a.source && a.source !== "builtin" && a.source !== "missing") tags.push(`<span class="tag plug">plugin</span>`);
  if (!a.declared) tags.push(`<span class="tag">undeclared</span>`);
  return tags.join(" ");
}

export function renderAgents() {
  // Filtered down to nothing is a fact about the selection. An instance that
  // declares no scopes at all is a different thing to say, and says it.
  const nothing = state.scope
    ? `No agents in ${esc(scopeLabel())}.`
    : "This instance declares no scopes.";
  $("agents").innerHTML = state.scopes.filter(s => inScope(s.name)).map(s => {
    const rows = s.agents.map(a => {
      const standing = a.lifetime !== "task";
      const live = a.state === "ready" || a.state === "starting";
      const buttons = [];
      if (standing) {
        buttons.push(`<button class="btn" data-act="start" data-scope="${esc(s.name)}" data-name="${esc(a.name)}" ${live ? "disabled" : ""}>Start</button>`);
        buttons.push(`<button class="btn" data-act="stop" data-id="${esc(a.id || "")}" ${live ? "" : "disabled"}>Stop</button>`);
        buttons.push(`<button class="btn" data-act="term" data-id="${esc(a.id || "")}" ${live ? "" : "disabled"}>Terminal</button>`);
      }
      buttons.push(`<button class="btn" data-act="task" data-scope="${esc(s.name)}" data-agent="${esc(a.adapter)}">Start task…</button>`);

      const rolePicker = standing && a.id
        ? `<select class="rolepick" data-act="role" data-id="${esc(a.id)}" title="What this agent is allowed to do">
             ${(state.roles || []).map(r => `
               <option value="${esc(r.name)}" ${r.name === a.role ? "selected" : ""}
                       title="${esc(r.describe)}">${esc(r.name)}</option>`).join("")}
             ${a.assigned_role ? `<option value="">use the config's</option>` : ""}
           </select>`
        : "";

      const attach = a.attach
        ? `<div class="attach"><span class="sub">attach</span><code>${esc(a.attach)}</code>
             <button class="btn" data-act="copy" data-copy="${esc(a.attach)}">Copy</button></div>`
        : (a.session ? `<div class="attach"><span class="sub">session</span><code>${esc(a.session)}</code></div>` : "");

      const work = a.active.length
        ? `<div class="work">${a.active.map(w => `
            <div class="job" data-task="${esc(w.task_id)}" data-run="${esc(w.run_id)}">
              ${statusBadge(w.status)} <span class="jt">${esc(w.task_title)}</span>
              <span class="sub">attempt ${w.attempt} · ${esc(w.trigger)} · ${esc(w.runtime)}${w.session ? " " + esc(w.session) : ""} · ${since(w.started_at)}</span>
            </div>`).join("")}</div>`
        : (standing && live
            ? `<div class="work idle">up and waiting${a.started_at ? " · " + since(a.started_at) : ""}</div>`
            : `<div class="work idle">nothing running</div>`);

      return `<div class="agent">
        <div class="line">
          <span class="nm">${esc(a.name)}</span>
          ${a.adapter !== a.name ? `<span class="sub">${esc(a.adapter)}</span>` : ""}
          ${standing ? statusBadge(a.state) : ""}
          ${agentTags(a)}
          <span style="margin-left:auto"></span>
          ${rolePicker}
          <span class="row-btns">${buttons.join("")}</span>
        </div>
        <div class="sub">${esc(a.description)}</div>
        ${a.error ? `<div class="err">${esc(a.error)}</div>` : ""}
        ${attach}
        ${work}
      </div>`;
    }).join("");

    return `<div class="scope">
      <div class="head">
        <h3>${esc(s.name)}</h3>
        <span class="sub">${esc(s.path)}</span>
        <span style="margin-left:auto"></span>
        <span class="sub">tasks default to ${esc(s.default_agent)} on ${esc(s.runtime)}</span>
      </div>
      ${rows || `<div class="empty">No agents declared.</div>`}
    </div>`;
  }).join("") || `<div class="empty">${nothing}</div>`;

  for (const el of $("agents").querySelectorAll("[data-act]")) {
    if (el.tagName === "SELECT") el.onchange = (e) => { e.stopPropagation(); agentAction(el); };
    else el.onclick = (e) => { e.stopPropagation(); agentAction(el); };
  }
  for (const job of $("agents").querySelectorAll(".job")) {
    job.onclick = () => openTask(job.dataset.task, job.dataset.run);
  }
}

export async function agentAction(el) {
  const act = el.dataset.act;
  try {
    if (act === "start") {
      el.disabled = true;
      await api("/api/agents/start", { method: "POST", body: JSON.stringify({ scope: el.dataset.scope, name: el.dataset.name }) });
    } else if (act === "stop") {
      el.disabled = true;
      await api("/api/agents/stop", { method: "POST", body: JSON.stringify({ id: el.dataset.id }) });
    } else if (act === "role") {
      el.disabled = true;
      await api("/api/agents/role", { method: "POST", body: JSON.stringify({ id: el.dataset.id, role: el.value }) });
    } else if (act === "term") {
      return openAgent(el.dataset.id);
    } else if (act === "task") {
      return openCreate({ scope: el.dataset.scope, agent: el.dataset.agent });
    } else if (act === "copy") {
      await navigator.clipboard.writeText(el.dataset.copy).catch(() => {});
      const was = el.textContent; el.textContent = "Copied"; setTimeout(() => { el.textContent = was; }, 1200);
      return;
    }
  } catch (e) {
    alert(e.message);
  }
  loadAgents();
}

export async function openAgent(id) {
  dropModal();
  const found = findAgent(id);
  scrim(`
    <header>
      <div><h2>${esc(found ? found.agent.name : id)}</h2>
           <code class="id">${esc(id)}</code></div>
      <button class="x" id="a-close">&times;</button>
    </header>
    <div class="body">
      <div class="row-btns">
        <button class="btn" id="a-stop">Stop</button>
        <button class="btn" id="a-restart">Restart</button>
      </div>
      <div class="err" id="a-err"></div>
      ${found && found.agent.attach
        ? `<div class="attach"><span class="sub">attach</span><code>${esc(found.agent.attach)}</code></div>` : ""}
      ${terminalBlock("Terminal")}
    </div>`);
  $("a-close").onclick = closeModal;
  $("a-stop").onclick = async () => {
    try { await api("/api/agents/stop", { method: "POST", body: JSON.stringify({ id }) }); closeModal(); loadAgents(); }
    catch (e) { $("a-err").textContent = e.message; }
  };
  $("a-restart").onclick = async () => {
    if (!found) return;
    try {
      await api("/api/agents/stop", { method: "POST", body: JSON.stringify({ id }) });
      await api("/api/agents/start", { method: "POST", body: JSON.stringify({ scope: found.scope, name: found.agent.name }) });
      loadAgents();
      setTerminal("agent", id, true);
    } catch (e) { $("a-err").textContent = e.message; }
  };
  wireTerminal();
  setTerminal("agent", id, true);
}

export function findAgent(id) {
  for (const s of state.scopes) {
    for (const a of s.agents) if (a.id === id) return { scope: s.name, agent: a };
  }
  return null;
}
