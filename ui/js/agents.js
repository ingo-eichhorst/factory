//! The roster: every scope, the agents it declares, what each one is doing, and
//! the buttons that start, stop and type at them -- per standing instance
//! (#330): its own Terminal, Connect and Stop / Start, and a word in the page
//! before stopping one that somebody would lose something by.

import { $, esc, api, state, since, statusBadge } from "./core.js";
import { inScope, scopeLabel } from "./scopes.js";
import { readinessTag } from "./sandboxes.js";
import { dateBadges } from "./dates.js";
import { scrim, closeModal, dropModal } from "./modal.js";
import { terminalBlock, wireTerminal, setTerminal } from "./terminal.js";
import { openTask } from "./tasks.js";
import { openCreate } from "./task-form.js";
import { agentConfigurePayload, agentDeletePayload } from "./agent-config.js";
import {
  canStart, connectLines, findInstance, instanceActions, instanceDetail, instanceSummary, instancesOf,
  isLive, liveIds, scalePayload, startInstancePayload, stopPrompt,
} from "./agents-model.js";
// The roster and the wiring open each other: the wiring builds the rail at
// boot, and this is the other place /api/agents is refetched, which is the
// only thing the rail's tree is made of. A cycle ES modules handle, because
// nothing here runs until a page is shown.
import { rebuildRail } from "./app.js";

/// Which instances have their Connect panel open, by id; kept across the
/// re-renders an event causes, so a panel does not close under the reader.
const connectOpen = new Set();
/// A stop waiting on the person's word (#330): `{ key, ids, prompt }`, shown
/// in place where its Stop button was. One at a time.
let confirming = null;

export async function loadAgents() {
  try {
    const board = await api("/api/agents");
    state.scopes = board.scopes;
    state.roles = board.roles || [];
    // Only the scopes whose roles differ from the instance's are listed; the
    // rest have exactly `roles`. See `rolesFor`.
    state.scopeRoles = board.scope_roles || {};
    // Served once alongside the scopes now, not copied onto each of them --
    // see `Payload::Scopes` -- so `available` comes off the board itself.
    state.adapters = board.available;
    // A scope added or dropped in the config has to reach the rail, not just
    // the roster underneath it.
    rebuildRail();
    renderAgents();
  } catch (e) {
    $("agents").innerHTML = `<div class="err">${esc(e.message)}</div>`;
  }
}


/// The roles in effect in one scope -- exactly what `set_agent_role` will
/// accept for an agent there, so the picker never offers a role the daemon
/// then refuses.
export function rolesFor(scope) {
  return (state.scopeRoles && state.scopeRoles[scope]) || state.roles || [];
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
  // The form on this page sets a sandbox, so this page has to show it, or an
  // agent declaring one is indistinguishable from an agent declaring none.
  // `warn` for a declaration nothing enforces (docker, srt), worth a second
  // look; `ok` for openshell, which dispatch enforces (#218). L2's Sandboxes
  // tab says the same thing at greater length.
  if (a.sandbox && a.sandbox !== "none") {
    tags.push(`<span class="tag ${a.sandbox === "openshell" ? "ok" : "warn"}">${esc(a.sandbox)}</span>`);
  }
  // Whether that sandbox could be made now (#234); the Sandboxes tab
  // names what is missing and the command that supplies it.
  tags.push(readinessTag(a).trim());
  if (a.source && a.source !== "builtin" && a.source !== "missing") tags.push(`<span class="tag plug">plugin</span>`);
  if (!a.declared) tags.push(`<span class="tag">undeclared</span>`);
  return tags.filter(Boolean).join(" ");
}

export function renderAgents() {
  // Filtered down to nothing is a fact about the selection. An instance with
  // no locally configured scopes is a different thing to say, and says it.
  const nothing = state.scope
    ? `No agents in ${esc(scopeLabel())}.`
    : "This instance has no configured scopes.";
  $("agents").innerHTML = state.scopes.filter(s => inScope(s.name)).map(s => {
    const rows = s.agents.map(a => {
      const standing = a.lifetime !== "task";
      const live = a.state === "ready" || a.state === "starting";
      // A standing agent may keep several standing sessions (#329). With one,
      // the row's buttons are that instance's; with several, Start brings up
      // whichever are down and Stop takes every live one down, and each
      // instance has its own line with its own buttons (#330).
      const summary = instanceSummary(a);
      const several = Boolean(summary && summary.several);
      const instances = standing ? instancesOf(a) : [];
      const only = !several ? instances[0] : null;
      const stopping = liveIds(a);
      const buttons = [];
      if (standing && only) {
        buttons.push(instanceButtons(s.name, a, only));
      } else if (standing) {
        buttons.push(`<button class="btn" data-act="start" data-scope="${esc(s.name)}" data-name="${esc(a.name)}" ${canStart(a) ? "" : "disabled"}>Start</button>`);
        buttons.push(`<button class="btn" data-act="stop" data-ids="${esc(stopping.join(" "))}" ${stopping.length ? "" : "disabled"}>Stop</button>`);
        if (!several) buttons.push(`<button class="btn" data-act="term" data-id="${esc(a.id || "")}" ${live ? "" : "disabled"}>Terminal</button>`);
      }
      // The count, and − / + where the declaration is this scope's own to
      // rewrite: the scale goes into its config, like Delete does.
      const scaler = summary && (several || a.deletable)
        ? `<span class="scaler" title="Standing sessions kept up: instances in the scope's config">
             <span class="sub">${esc(summary.label)}</span>
             ${a.deletable ? `
               <button class="btn" data-act="scale" data-by="-1" data-scope="${esc(s.name)}" data-name="${esc(a.name)}" data-wanted="${summary.wanted}" ${summary.wanted > 1 ? "" : "disabled"} title="One fewer">−</button>
               <button class="btn" data-act="scale" data-by="1" data-scope="${esc(s.name)}" data-name="${esc(a.name)}" data-wanted="${summary.wanted}" title="One more">+</button>` : ""}
           </span>`
        : "";
      buttons.push(`<button class="btn" data-act="task" data-scope="${esc(s.name)}" data-agent="${esc(a.adapter)}">Start task…</button>`);
      if (a.deletable && state.scope === s.name)
        buttons.push(`<button class="btn danger" data-act="delete" data-scope="${esc(s.name)}" data-name="${esc(a.name)}">Delete</button>`);

      const rolePicker = standing && a.id
        ? `<select class="rolepick" data-act="role" data-id="${esc(a.id)}" title="What this agent is allowed to do">
             ${rolesFor(s.name).map(r => `
               <option value="${esc(r.name)}" ${r.name === a.role ? "selected" : ""}
                       title="${esc(r.describe)}">${esc(r.name)}</option>`).join("")}
             ${a.assigned_role ? `<option value="">use the config's</option>` : ""}
           </select>`
        : "";

      // One standing session: the row is the instance, with its id, uptime
      // and whatever is open under it. Several: a line per instance, each
      // with its own id, state, uptime and buttons.
      const attach = several
        ? `<div class="instances">${instances.map(i => `
            <div class="instance">
              <div class="line">
                <span class="sub">#${esc(String(i.instance))}</span>
                <code class="id">${esc(i.id)}</code>
                ${statusBadge(i.state)}
                ${i.surplus ? `<span class="tag warn" title="Above the count: stopped once nobody is using it">draining</span>` : ""}
                ${seenTag(i)}
                <span class="sub">${esc(instanceDetail(i))}</span>
                <span style="margin-left:auto"></span>
                <span class="row-btns">${instanceButtons(s.name, a, i)}</span>
              </div>
              ${i.error ? `<div class="err">${esc(i.error)}</div>` : ""}
              ${instanceExtras(i)}
            </div>`).join("")}</div>`
        : only
          ? `<div class="instance one">
               <div class="line"><code class="id">${esc(only.id)}</code>${seenTag(only)}
                 <span class="sub">${esc(instanceDetail(only))}</span></div>
               ${instanceExtras(only)}
             </div>`
          : "";

      const work = a.active.length
        ? `<div class="work">${a.active.map(w => `
            <div class="job" data-task="${esc(w.task_id)}" data-run="${esc(w.run_id)}">
              ${statusBadge(w.status)} <span class="jt">${esc(w.task_title)}</span>
              <span class="sub">attempt ${w.attempt} · ${esc(w.trigger)} · ${esc(w.runtime)}${w.session ? " " + esc(w.session) : ""} · ${since(w.started_at)}</span>
            </div>`).join("")}</div>`
        : (standing && instances.length
            // Each instance's own line already says how long it has been up.
            ? ""
            : standing && live
              ? `<div class="work idle">up and waiting${a.started_at ? " · " + since(a.started_at) : ""}</div>`
              : `<div class="work idle">nothing running</div>`);

      return `<div class="agent">
        <div class="line">
          <span class="nm">${esc(a.name)}</span>
          ${a.adapter !== a.name ? `<span class="sub">${esc(a.adapter)}</span>` : ""}
          ${standing ? statusBadge(a.state) : ""}
          ${agentTags(a)}
          ${dateBadges({ scope: s.name, agent: a.name })}
          <span style="margin-left:auto"></span>
          ${scaler}
          ${rolePicker}
          <span class="row-btns">${buttons.join("")}</span>
        </div>
        <div class="sub">${esc(a.description)}</div>
        ${a.error && !several ? `<div class="err">${esc(a.error)}</div>` : ""}
        ${confirming && several && confirming.key === stopping.join(" ") ? confirmBlock(confirming) : ""}
        ${attach}
        ${work}
      </div>`;
    }).join("");

    return `<div class="scope">
      <div class="head">
        <h3>${esc(s.name)}</h3>
        <span class="sub">${esc(s.path)}</span>
        <code class="id" title="Stable scope ID">${esc(s.id)}</code>
        <span style="margin-left:auto"></span>
        <span class="sub">tasks default to ${esc(s.default_agent)} on ${esc(s.runtime)}, kept in ${esc(s.task_store)}</span>
        ${state.scope === s.name ? `<button class="btn primary" data-act="configure" data-scope="${esc(s.name)}">New agent…</button>` : ""}
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
      // One instance's Start names it; the row's starts every instance it keeps.
      const body = el.dataset.instance
        ? startInstancePayload(el.dataset.scope, el.dataset.name, el.dataset.instance)
        : { scope: el.dataset.scope, name: el.dataset.name };
      await api("/api/agents/start", { method: "POST", body: JSON.stringify(body) });
    } else if (act === "stop") {
      el.disabled = true;
      const ids = (el.dataset.ids || "").split(" ").filter(Boolean);
      // Asked first, stopped only if nobody would lose anything -- otherwise
      // the page says who or what, and waits for the person's word.
      const prompt = await stopCost(ids);
      if (prompt) {
        confirming = { key: el.dataset.ids, ids, prompt };
        renderAgents();
        return;
      }
      await stopAll(ids);
    } else if (act === "stop-confirm") {
      const ids = confirming ? confirming.ids : [];
      confirming = null;
      el.disabled = true;
      await stopAll(ids);
    } else if (act === "stop-cancel") {
      confirming = null;
      renderAgents();
      return;
    } else if (act === "connect") {
      const id = el.dataset.id;
      if (connectOpen.has(id)) {
        connectOpen.delete(id);
      } else {
        connectOpen.add(id);
        // Shown and copied in one press: the first line is the one to paste.
        const found = findInstance(state.scopes, id);
        const first = found && connectLines(found.instance)[0];
        if (first) await navigator.clipboard?.writeText(first.command).catch(() => {});
      }
      renderAgents();
      return;
    } else if (act === "scale") {
      const payload = scalePayload(el.dataset.scope, el.dataset.name, Number(el.dataset.by), Number(el.dataset.wanted));
      el.disabled = true;
      const answer = await api("/api/agents/scale", { method: "POST", body: JSON.stringify(payload) });
      // Never forced: what a scale-down could not stop is said, not hidden.
      const scale = answer && answer.scale;
      if (scale && scale.kept && scale.kept.length) {
        alert(scale.kept.map(k => `${k.id} is still up: ${keptWords(k.why)}. It stops once it is idle.`).join("\n"));
      }
    } else if (act === "role") {
      el.disabled = true;
      await api("/api/agents/role", { method: "POST", body: JSON.stringify({ id: el.dataset.id, role: el.value }) });
    } else if (act === "term") {
      return openAgent(el.dataset.id);
    } else if (act === "task") {
      return openCreate({ scope: el.dataset.scope, agent: el.dataset.agent });
    } else if (act === "configure") {
      return openAgentCreate(el.dataset.scope);
    } else if (act === "delete") {
      const payload = agentDeletePayload(el.dataset.scope, el.dataset.name);
      if (!confirm(`Delete ${payload.name} from ${payload.scope}? Any standing session will be stopped.`)) return;
      el.disabled = true;
      await api("/api/agents/configure", {
        method: "DELETE",
        body: JSON.stringify(payload),
      });
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

/// The buttons on one instance's line: Terminal, Connect, and Stop or Start
/// for that instance only.
function instanceButtons(scope, agent, instance) {
  const can = instanceActions(instance);
  const id = esc(instance.id);
  const why = can.connect
    ? "Show and copy the command that attaches a terminal to it"
    : (isLive(instance) ? "A shell pane has no named agent to attach to: use Terminal" : "Not up");
  return [
    `<button class="btn" data-act="term" data-id="${id}" ${can.terminal ? "" : "disabled"}>Terminal</button>`,
    `<button class="btn" data-act="connect" data-id="${id}" title="${esc(why)}" ${can.connect ? "" : "disabled"}>Connect</button>`,
    can.stop
      ? `<button class="btn" data-act="stop" data-ids="${id}">Stop</button>`
      : `<button class="btn" data-act="start" data-scope="${esc(scope)}" data-name="${esc(agent.name)}" data-instance="${esc(String(instance.instance))}">Start</button>`,
  ].join("");
}

/// A person seen at it within the last minute, said on its line.
function seenTag(instance) {
  return instance.attended ? `<span class="tag warn" title="${esc(instance.attended)}">attended</span>` : "";
}

/// What opens under one instance: its Connect panel, and a stop waiting on a
/// confirmation, if either is.
function instanceExtras(instance) {
  let out = "";
  if (connectOpen.has(instance.id)) {
    const lines = connectLines(instance);
    out += `<div class="connect">${lines.map(l => `
      <div class="attach"><span class="sub">${esc(l.label)}</span><code>${esc(l.command)}</code>
        <button class="btn" data-act="copy" data-copy="${esc(l.command)}">Copy</button></div>`).join("")
      || `<div class="sub">Nothing to attach with.</div>`}</div>`;
  }
  if (confirming && confirming.key === instance.id) out += confirmBlock(confirming);
  return out;
}

/// The page's own confirmation (never `window.confirm`): what stopping would
/// cost, line by line, and the two ways out.
function confirmBlock(pending) {
  return `<div class="confirm-stop">
    <b>${esc(pending.prompt.title)}</b>
    ${pending.prompt.lines.map(l => `<div>${esc(l)}</div>`).join("")}
    <div class="row-btns">
      <button class="btn danger" data-act="stop-confirm">Stop anyway</button>
      <button class="btn" data-act="stop-cancel">Keep it running</button>
    </div>
  </div>`;
}

/// What stopping these instances would cost, asked of the daemon now; `null`
/// when nothing.
async function stopCost(ids) {
  const checks = [];
  for (const id of ids) {
    const answer = await api("/api/agents/stop-check", { method: "POST", body: JSON.stringify({ id }) });
    if (answer && answer.check) checks.push(answer.check);
  }
  return stopPrompt(checks);
}

async function stopAll(ids) {
  for (const id of ids) {
    await api("/api/agents/stop", { method: "POST", body: JSON.stringify({ id }) });
  }
}

function keptWords(why) {
  return { working: "it is working", blocked: "it is waiting on an answer", attached: "a person is attached to it" }[why] || why;
}

export function openAgentCreate(scope) {
  if (!scope || state.scope !== scope) return;
  dropModal();
  const adapters = state.adapters.map(adapter =>
    `<option value="${esc(adapter)}">${esc(adapter)}</option>`).join("");
  const roles = rolesFor(scope).map(role =>
    `<option value="${esc(role.name)}" ${role.name === "worker" ? "selected" : ""} title="${esc(role.describe)}">${esc(role.name)}</option>`).join("");
  scrim(`
    <header><div><h2>New agent</h2><code class="id">${esc(scope)}</code></div>
      <button class="x" id="na-close">&times;</button></header>
    <div class="body">
      <div class="grid2">
        <div><label for="na-name">Name <span class="sub" style="text-transform:none">(blank = harness)</span></label>
          <input id="na-name" placeholder="reviewer"></div>
        <div><label for="na-harness">Harness</label>
          <select id="na-harness">${adapters}</select></div>
      </div>
      <div class="grid2">
        <div><label for="na-lifetime">Lifetime</label>
          <select id="na-lifetime">
            <option value="task">task</option>
            <option value="temporary">temporary</option>
            <option value="permanent">permanent</option>
          </select></div>
        <div><label for="na-role">Role</label>
          <select id="na-role">${roles}</select></div>
      </div>
      <label for="na-sandbox">Sandbox <span class="sub" style="text-transform:none">(docker and srt are declared only; openshell is enforced and set in the scope's config file)</span></label>
      <select id="na-sandbox">
        <option value="none" selected>none</option>
        <option value="docker">docker</option>
        <option value="srt">srt</option>
      </select>
      <label for="na-autostart">Autostart</label>
      <div class="checkrow">
        <input type="checkbox" id="na-autostart" disabled>
        <span id="na-autostart-note">Task agents start with their tasks.</span>
      </div>
      <label for="na-args">CLI arguments <span class="sub" style="text-transform:none">(one argument per line, in order)</span></label>
      <textarea id="na-args" placeholder="--model&#10;opus"></textarea>
      <div class="err" id="na-err"></div>
      <div class="row-btns" style="margin-top:16px">
        <button class="btn primary" id="na-create">Create agent</button>
        <button class="btn" id="na-cancel">Cancel</button>
      </div>
    </div>`);
  $("na-close").onclick = closeModal;
  $("na-cancel").onclick = closeModal;
  $("na-lifetime").onchange = syncAgentAutostart;
  $("na-create").onclick = () => createAgent(scope);
  $("na-name").focus();
}

function syncAgentAutostart() {
  const lifetime = $("na-lifetime").value;
  const autostart = $("na-autostart");
  autostart.disabled = lifetime === "task";
  autostart.checked = lifetime === "permanent";
  $("na-autostart-note").textContent = lifetime === "task"
    ? "Task agents start with their tasks."
    : lifetime === "permanent"
      ? "Start now and restart it if its session disappears."
      : "Start it only when somebody asks.";
}

async function createAgent(scope) {
  const button = $("na-create");
  $("na-err").textContent = "";
  try {
    const payload = agentConfigurePayload(scope, {
      name: $("na-name").value,
      harness: $("na-harness").value,
      lifetime: $("na-lifetime").value,
      role: $("na-role").value,
      sandbox: $("na-sandbox").value,
      autostart: $("na-autostart").checked,
      arguments: $("na-args").value,
    });
    button.disabled = true;
    await api("/api/agents/configure", {
      method: "POST",
      body: JSON.stringify(payload),
    });
    closeModal();
    await loadAgents();
  } catch (e) {
    button.disabled = false;
    $("na-err").textContent = e.message;
  }
}

/// One instance's terminal (#330): the live web terminal for that instance's
/// session alone, with its own Stop and Restart and the commands to attach
/// from a shell. Stop asks first, like the roster's.
export async function openAgent(id) {
  dropModal();
  const found = findInstance(state.scopes, id);
  const instance = found ? found.instance : null;
  const several = Boolean(found && instanceSummary(found.agent)?.several);
  const title = found ? `${found.agent.name}${several ? ` #${instance.instance}` : ""}` : id;
  scrim(`
    <header>
      <div><h2>${esc(title)}</h2>
           <code class="id">${esc(id)}</code></div>
      <button class="x" id="a-close">&times;</button>
    </header>
    <div class="body">
      <div class="row-btns">
        <button class="btn" id="a-stop">Stop</button>
        <button class="btn" id="a-restart">Restart</button>
      </div>
      <div class="err" id="a-err"></div>
      <div id="a-confirm"></div>
      ${instance ? connectLines(instance).map(l => `
        <div class="attach"><span class="sub">${esc(l.label)}</span><code>${esc(l.command)}</code>
          <button class="btn" data-copy="${esc(l.command)}">Copy</button></div>`).join("") : ""}
      ${terminalBlock("Terminal")}
    </div>`);
  $("a-close").onclick = closeModal;
  for (const button of document.querySelectorAll(".modal [data-copy]")) {
    button.onclick = () => navigator.clipboard?.writeText(button.dataset.copy).catch(() => {});
  }
  const stop = async () => {
    await api("/api/agents/stop", { method: "POST", body: JSON.stringify({ id }) });
    closeModal();
    loadAgents();
  };
  $("a-stop").onclick = async () => {
    try {
      const prompt = await stopCost([id]);
      if (!prompt) return await stop();
      $("a-confirm").innerHTML = confirmBlock({ prompt });
      $("a-confirm").querySelector('[data-act="stop-confirm"]').onclick = () => stop().catch(e => { $("a-err").textContent = e.message; });
      $("a-confirm").querySelector('[data-act="stop-cancel"]').onclick = () => { $("a-confirm").innerHTML = ""; };
    } catch (e) { $("a-err").textContent = e.message; }
  };
  // This instance only: stopped and brought back by its own number.
  $("a-restart").onclick = async () => {
    if (!found) return;
    try {
      await api("/api/agents/stop", { method: "POST", body: JSON.stringify({ id }) });
      await api("/api/agents/start", {
        method: "POST",
        body: JSON.stringify(startInstancePayload(found.scope, found.agent.name, instance.instance)),
      });
      loadAgents();
      setTerminal("agent", id, true);
    } catch (e) { $("a-err").textContent = e.message; }
  };
  wireTerminal();
  setTerminal("agent", id, true);
}

/// The roster row an id belongs to -- instance 1's, or any other instance's.
export function findAgent(id) {
  const found = findInstance(state.scopes, id);
  return found ? { scope: found.scope, agent: found.agent } : null;
}
