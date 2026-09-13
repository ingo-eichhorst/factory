//! The L3 Roles view: every role in effect in the selected scope -- what it is
//! for, what it may and may not do, how far that reaches, where it was
//! written, and who holds it. With All scopes selected, where roles are
//! written across the tree instead.
//!
//! Nothing here decides anything. `authorize` checks roles against the same
//! definitions `/api/roles` serves, and the grant phrases come from the daemon
//! too, so this page cannot drift into its own vocabulary. It says the rest out
//! loud, including that roles are guard-rails and not a security boundary --
//! that sentence is in the page's markup, so a failed fetch cannot remove it.

import { $, api, esc, state } from "./core.js";
import { scrim, closeModal, dropModal } from "./modal.js";
import { routeHref } from "./scopes.js";

/// Answers can arrive out of order when the rail moves quickly; only the
/// newest request is allowed to draw.
let asked = 0;

// ------------------------------------------------------------ pure helpers

/// A scope's Roles view, so a badge or a layer header can link straight to it.
export function rolesHref(scope) {
  return routeHref(scope, "roles");
}

function sameOrigin(a, b) {
  if (!a || !b || a.kind !== b.kind) return false;
  return a.kind !== "scope" || a.scope === b.scope;
}

function originName(origin) {
  if (!origin) return "";
  if (origin.kind === "builtin") return "built-in";
  if (origin.kind === "instance") return "instance";
  return origin.scope;
}

/// Where a role came from, relative to the scope being looked at. `writes` is
/// the layer that scope writes -- the board says which, so the page never has
/// to work out from paths whether it is looking at the instance root.
export function originBadge(role, writes) {
  const origin = role.origin || { kind: "builtin" };
  if (origin.kind === "builtin") {
    return { text: "built-in", cls: "builtin", here: false };
  }
  if (sameOrigin(origin, writes)) {
    const text = role.overrides && role.overrides.kind !== "builtin"
      ? `defined here · overrides ${originName(role.overrides)}`
      : "defined here";
    return { text, cls: "here", here: true };
  }
  if (origin.kind === "instance") {
    return { text: "instance", cls: "instance", here: false };
  }
  return { text: `inherited from ${origin.scope}`, cls: "inherited", here: false, link: origin.scope };
}

/// Reach in words, and the one rule grants cannot express.
export function reachText(reach, scope) {
  if (reach === "scope") {
    return `everything in ${scope || "its own scope"}, never past it`;
  }
  return "only its own work: tasks assigned to it, the run it holds, itself. " +
    "It may change what its task says, never whose it is";
}

/// A role's grants against the whole vocabulary, grouped as the daemon groups
/// them, so a grant a role lacks is shown missing rather than left out.
export function grantGroups(role, grants) {
  const held = new Set(role.grants || []);
  const groups = [];
  for (const grant of grants || []) {
    let group = groups.find(g => g.group === grant.group);
    if (!group) {
      group = { group: grant.group, items: [] };
      groups.push(group);
    }
    group.items.push({ name: grant.name, describe: grant.describe, allowed: held.has(grant.name) });
  }
  return groups;
}

/// The body `/api/roles` expects for a define. Throws a sentence a person can
/// act on; the daemon checks everything again.
export function roleDefinePayload(scope, values, replace) {
  const target = String(scope ?? "").trim();
  if (!target) throw new Error("Select one scope before defining a role in it.");
  const name = String(values.name ?? "").trim();
  if (!name) throw new Error("A role needs a name.");
  if (!/^[A-Za-z0-9_.-]+$/.test(name)) {
    throw new Error("A role name is letters, digits, -, _ and ., with no spaces.");
  }
  if (name === "worker" || name === "foreman") {
    throw new Error(`${name} is built in and cannot be redefined.`);
  }
  const reach = String(values.reach ?? "own");
  if (!["own", "scope"].includes(reach)) throw new Error("Choose own or scope reach.");
  const describe = String(values.describe ?? "").trim();
  const role = { grants: [...new Set(values.grants || [])], reach };
  if (describe) role.describe = describe;
  return { scope: target, name, role, replace: Boolean(replace) };
}

// ----------------------------------------------------------------- markup

function holders(role) {
  if (!role.held_by || !role.held_by.length) return `<span class="sub">nobody</span>`;
  return role.held_by.map(h =>
    `<span class="role-holder">${esc(h.name)}${h.given ? ` <span class="tag" title="given with factory agent role, not declared in the config">given</span>` : ""}</span>`
  ).join("");
}

function badge(role, writes) {
  const b = originBadge(role, writes);
  const text = esc(b.text);
  return b.link
    ? `<a class="role-origin ro-${b.cls}" href="${rolesHref(b.link)}" title="open the scope that defines it">${text}</a>`
    : `<span class="role-origin ro-${b.cls}">${text}</span>`;
}

/// One role. `opts.scope` is the scope it is being shown for (null for the
/// tree), `opts.writes` the layer that scope writes, and `opts.actions` whether
/// to offer the owner's buttons at all.
export function roleCard(role, grants, opts = {}) {
  const b = originBadge(role, opts.writes);
  const rows = grantGroups(role, grants).map(g => `
    <div class="role-grants">
      <span class="role-group">${esc(g.group)}</span>
      <span class="role-marks">${g.items.map(i =>
        `<span class="grant ${i.allowed ? "yes" : "no"}" title="${esc(i.name)}"><span aria-hidden="true">${i.allowed ? "✓" : "✗"}</span> ${esc(i.describe)}<span class="vh">${i.allowed ? " (may)" : " (may not)"}</span></span>`
      ).join("")}</span>
    </div>`).join("");

  let buttons = "";
  if (opts.actions && b.cls !== "builtin") {
    buttons = b.here
      ? `<button class="btn" data-role-act="edit" data-name="${esc(role.name)}">Edit</button>
         <button class="btn danger" data-role-act="delete" data-name="${esc(role.name)}">Delete</button>`
      : `<button class="btn" data-role-act="override" data-name="${esc(role.name)}">Override here</button>`;
  }

  return `<article class="role-card ${b.here ? "here" : ""} ${b.cls === "inherited" || b.cls === "instance" ? "inherited" : ""}" data-role="${esc(role.name)}">
    <div class="role-head">
      <h3>${esc(role.name)}</h3>
      ${badge(role, opts.writes)}
      <span class="sp"></span>
      ${buttons ? `<span class="row-btns">${buttons}</span>` : ""}
    </div>
    <p class="role-describe">${esc(role.describe)}</p>
    ${rows}
    <div class="role-grants"><span class="role-group">Reach</span><span>${esc(reachText(role.reach, opts.scope))}</span></div>
    ${opts.scope ? `<div class="role-grants"><span class="role-group">Held by</span><span class="role-holders">${holders(role)}</span></div>` : ""}
  </article>`;
}

/// Roles against grants, compact, for comparing them at a glance.
export function roleMatrix(roles, grants) {
  if (!roles.length) return "";
  const head = (grants || []).map(g => `<th title="${esc(g.describe)}"><span>${esc(g.name)}</span></th>`).join("");
  const body = roles.map(r => {
    const held = new Set(r.grants || []);
    return `<tr><th scope="row">${esc(r.name)}</th>${(grants || []).map(g =>
      `<td class="${held.has(g.name) ? "yes" : "no"}">${held.has(g.name) ? "✓" : "·"}</td>`).join("")}
      <td class="reach">${esc(r.reach)}</td></tr>`;
  }).join("");
  return `<div class="role-matrix"><table>
    <thead><tr><th></th>${head}<th>reach</th></tr></thead>
    <tbody>${body}</tbody>
  </table></div>`;
}

/// What a scope layer adds, and what it replaces.
function layerRole(role) {
  const verb = role.overrides && role.overrides.kind !== "builtin"
    ? `replaces ${esc(originName(role.overrides))}'s`
    : "adds";
  return `<li><span class="role-verb">${verb}</span> <strong>${esc(role.name)}</strong>
    <span class="sub">${esc(role.describe)} · ${esc((role.grants || []).join(", ") || "no grants")} · ${esc(role.reach)} reach</span></li>`;
}

/// The inheritance itself, for All scopes: Factory's roles and the instance's
/// once, then every scope that writes roles, nested under the nearest scope
/// above it that also does.
export function layerTree(board) {
  const layers = board.layers || [];
  const builtin = layers.find(l => l.origin.kind === "builtin");
  const instance = layers.find(l => l.origin.kind === "instance");
  const scoped = layers.filter(l => l.origin.kind === "scope");

  const children = (parent) => scoped.filter(l => (l.parent || null) === parent);
  const branch = (parent, depth) => children(parent).map(l => `
    <section class="role-layer" style="--d:${depth}">
      <h3><a href="${rolesHref(l.origin.scope)}">${esc(l.origin.scope)}</a></h3>
      <ul>${l.roles.map(layerRole).join("")}</ul>
    </section>
    ${branch(l.origin.scope, depth + 1)}`).join("");

  const cards = (layer) => (layer ? layer.roles : []).map(r => roleCard(r, board.grants, {})).join("");
  const tree = branch(null, 0);
  return `
    <h3 class="role-section">Built in</h3>
    <div class="role-cards">${cards(builtin)}</div>
    <h3 class="role-section">Instance</h3>
    ${instance && instance.roles.length
      ? `<div class="role-cards">${cards(instance)}</div>`
      : `<div class="empty">The instance root's config defines no roles of its own.</div>`}
    <h3 class="role-section">Scopes</h3>
    ${tree || `<div class="empty">No scope defines roles of its own. Every scope has exactly the roles above.</div>`}`;
}

/// Scopes below the selected one that write roles, so its view is not taken
/// for the whole tree.
function descendants(board) {
  const selected = state.scopes.find(s => s.name === board.scope);
  if (!selected) return "";
  const base = String(selected.path || "").replace(/\/+$/, "");
  const below = (board.layers || []).filter(l =>
    l.origin.kind === "scope" && l.origin.scope !== board.scope &&
    l.path && base && l.path.startsWith(`${base}/`));
  if (!below.length) return "";
  return `<div class="env-note role-below">Scopes below ${esc(board.scope)} add or override roles of their own:
    ${below.map(l => `<a href="${rolesHref(l.origin.scope)}">${esc(l.origin.scope)}</a>
      <span class="sub">(${l.roles.map(r => esc(r.name)).join(", ")})</span>`).join(" · ")}</div>`;
}

// ------------------------------------------------------------------- view

export function renderRoles() {
  const el = $("roles");
  if (!el) return;
  const newButton = $("roles-new");
  if (newButton) {
    newButton.disabled = state.scope === null;
    newButton.title = state.scope === null
      ? "Select one scope in the rail to define a role in it"
      : `Define a role in ${state.scope}`;
  }
  if (state.roleBoardError) {
    el.innerHTML = `<div class="err runtime-load-error">${esc(state.roleBoardError)}</div>`;
    return;
  }
  const board = state.roleBoard;
  if (!board) { el.innerHTML = `<div class="empty">loading…</div>`; return; }

  if (!board.scope) {
    el.innerHTML = layerTree(board);
    return;
  }

  const roles = board.roles || [];
  // Defined here first, then inherited, then what ships -- the order a person
  // editing this scope cares about.
  const rank = r => {
    const b = originBadge(r, board.writes);
    return b.here ? 0 : b.cls === "builtin" ? 2 : 1;
  };
  const ordered = [...roles].sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name));
  el.innerHTML = `
    <p class="role-read">Every role may read the board: tasks, runs, agents and workflows. What differs is below.</p>
    ${roleMatrix(ordered, board.grants)}
    ${descendants(board)}
    <div class="role-cards">${ordered.map(r => roleCard(r, board.grants, {
      scope: board.scope, writes: board.writes, actions: true,
    })).join("")}</div>`;

  for (const button of el.querySelectorAll("[data-role-act]")) {
    button.onclick = () => roleAction(button.dataset.roleAct, button.dataset.name);
  }
}

export async function loadRoles() {
  const mine = ++asked;
  const query = state.scope === null ? "" : `?scope=${encodeURIComponent(state.scope)}`;
  try {
    const answer = await api(`/api/roles${query}`);
    if (mine !== asked) return;
    state.roleBoard = answer.board;
    state.roleBoardError = null;
  } catch (e) {
    if (mine !== asked) return;
    state.roleBoard = null;
    state.roleBoardError = e.message;
  }
  renderRoles();
}

export function wireRoles() {
  $("roles-refresh").onclick = () => loadRoles();
  $("roles-new").onclick = () => openRoleForm("new");
}

// ---------------------------------------------------- the owner's changes

function findRole(name) {
  return (state.roleBoard?.roles || []).find(r => r.name === name);
}

async function roleAction(act, name) {
  if (act === "edit") return openRoleForm("edit", findRole(name));
  if (act === "override") return openRoleForm("override", findRole(name));
  if (act === "delete") {
    const scope = state.roleBoard?.scope;
    if (!scope || !confirm(`Delete the role ${name} from ${scope}? Agents below it that inherit a definition from further up keep that one.`)) return;
    try {
      await api("/api/roles", { method: "DELETE", body: JSON.stringify({ scope, name }) });
    } catch (e) {
      alert(e.message);
    }
    loadRoles();
  }
}

/// New role…, Edit, or Override here. Grants and reach are picked, never
/// typed: the daemon's own list is the only list.
export function openRoleForm(mode, role) {
  const board = state.roleBoard;
  const scope = board?.scope;
  if (!board || !scope || state.scope !== scope) return;
  dropModal();
  const held = new Set(role ? role.grants : []);
  const groups = grantGroups({ grants: [] }, board.grants).map(g => `
    <fieldset class="role-pick">
      <legend>${esc(g.group)}</legend>
      ${g.items.map(i => `<label class="checkrow"><input type="checkbox" name="rf-grant" value="${esc(i.name)}" ${held.has(i.name) ? "checked" : ""}>
        <span>${esc(i.describe)} <code class="id">${esc(i.name)}</code></span></label>`).join("")}
    </fieldset>`).join("");
  const reach = role ? role.reach : "own";
  const title = mode === "edit" ? "Edit role" : mode === "override" ? "Override role" : "New role";
  const note = mode === "override"
    ? `<p class="env-note">This copies ${esc(role.name)} from ${esc(originName(role.origin))} into ${esc(scope)}. Saving replaces the inherited definition here and in every scope below, whole; ${esc(originName(role.origin))} keeps its own.</p>`
    : "";

  scrim(`
    <header><div><h2>${title}</h2><code class="id">${esc(scope)}</code></div>
      <button class="x" id="rf-close">&times;</button></header>
    <div class="body">
      ${note}
      <label for="rf-name">Name</label>
      <input id="rf-name" value="${esc(role ? role.name : "")}" placeholder="reviewer" ${role ? "disabled" : ""}>
      <label for="rf-describe">What it is for</label>
      <input id="rf-describe" value="${esc(role && mode !== "new" ? role.describe : "")}" placeholder="works its own tasks and says what it found">
      <label>What it may do</label>
      <div class="role-picks">${groups}</div>
      <label>Reach</label>
      <label class="checkrow"><input type="radio" name="rf-reach" value="own" ${reach === "own" ? "checked" : ""}>
        <span>own — ${esc(reachText("own"))}</span></label>
      <label class="checkrow"><input type="radio" name="rf-reach" value="scope" ${reach === "scope" ? "checked" : ""}>
        <span>scope — ${esc(reachText("scope", scope))}</span></label>
      <p class="sub role-form-note">A change holds from each agent's next request. A guide already in a running session is not rewritten.</p>
      <div class="err" id="rf-err"></div>
      <div class="row-btns" style="margin-top:16px">
        <button class="btn primary" id="rf-save">${mode === "edit" ? "Save" : mode === "override" ? "Override here" : "Create role"}</button>
        <button class="btn" id="rf-cancel">Cancel</button>
      </div>
    </div>`);
  $("rf-close").onclick = closeModal;
  $("rf-cancel").onclick = closeModal;
  $("rf-save").onclick = () => saveRole(scope, mode);
  (role ? $("rf-describe") : $("rf-name")).focus();
}

async function saveRole(scope, mode) {
  const button = $("rf-save");
  $("rf-err").textContent = "";
  try {
    const payload = roleDefinePayload(scope, {
      name: $("rf-name").value,
      describe: $("rf-describe").value,
      grants: [...document.querySelectorAll('input[name="rf-grant"]:checked')].map(i => i.value),
      reach: (document.querySelector('input[name="rf-reach"]:checked') || {}).value,
    }, mode === "edit");
    button.disabled = true;
    await api("/api/roles", { method: "POST", body: JSON.stringify(payload) });
    closeModal();
    await loadRoles();
  } catch (e) {
    button.disabled = false;
    $("rf-err").textContent = e.message;
  }
}
