//! L2 Environment, tab 2: the declared secrets catalogue (#244) -- each
//! entry's presence, whether it resolves, its expiry and who uses it, and
//! the one edit (its expiry, renew line and note) -- then the correction that
//! Factory injects no credentials, and the honest inventory of what an agent
//! can already reach because it runs as the daemon's owner. Reads the same
//! `/api/environment` answer `sandboxes.js` fetches -- see `loadEnvironment`
//! there. Never a value, here or anywhere below.

import { $, api, esc, state } from "./core.js";
import { inScope } from "./scopes.js";
import {
  expiryClass, expiryText, formValues, metadataBody, presenceText, resolutionText, stateText, useText,
  validDate, visibleUndeclared,
} from "./secrets-model.js";

const CORRECTION = "Factory injects no credentials into a session -- the only environment it adds is six " +
  "FACTORY_* variables (scope, socket, binary, token, and a run's task id and attempt). That is not the same " +
  "as the agent having none: it runs as the owner, in the owner's home, and can already reach anything below.";

const BOUNDARY = "Values are never shown here. The well-known locations below are paths checked for existence, " +
  "nothing more. A declared secret's source is read only by the provisioner -- to hand the value to the one " +
  "OpenShell provider that needs it, and to learn whether it resolves -- and what this page shows of it is yes or " +
  "no. The only thing it can change is a declared secret's expiry, renew line and note, in the root config.";

// A fact about how the daemon runs its agents, not about the answer to any
// one request -- so it lives here rather than in the payload, and is still on
// screen when the fetch fails. That is the moment it matters most: a page that
// cannot list what is reachable must not also stop saying that everything is.
const REACHABILITY = "Every agent runs as the daemon's owner, in the owner's home, so every credential below " +
  "that is present is already reachable by every agent on this machine -- Factory injects none of them, and " +
  "nothing here narrows what an agent can reach.";

/// Narrow the inventory to the rail's inclusive scope selection, the same way
/// `sandboxes.js` narrows its own rows.
///
/// A row with no scope is a credential in the owner's home. It belongs to no
/// scope and is reachable from every one of them, so it survives every
/// selection -- hiding it under a scope filter would have the page quietly
/// unsay the one thing this tab exists to say.
export function visibleCredentials(rows, contains = inScope) {
  return (rows || []).filter(row => !row.scope || contains(row.scope));
}

function credentialRow(row) {
  return `<tr>
    <td>${esc(row.integration)}</td>
    <td>${esc(row.label)}</td>
    <td><code>${esc(row.path)}</code></td>
    <td>${row.present ? `<span class="tag warn">present</span>` : `<span class="tag">absent</span>`}</td>
  </tr>`;
}

function tag(text, cls) {
  return `<span class="tag ${esc(cls || "")}">${esc(text)}</span>`;
}

function catalogueRow(row) {
  const presence = presenceText(row);
  const resolution = resolutionText(row);
  const users = (row.used_by || []).map(u => `<div>${esc(useText(u))}</div>`).join("") || `<span class="sub">nothing names it</span>`;
  return `<tr>
    <td><b>${esc(row.name)}</b><div class="sub">${esc(row.kind)}</div>${row.note ? `<div class="sub">${esc(row.note)}</div>` : ""}</td>
    <td><code>${esc(row.source)}</code></td>
    <td>${tag(presence.text, presence.cls)}</td>
    <td>${tag(resolution.text, resolution.cls)}</td>
    <td>${tag(stateText(row.state), expiryClass(row.state))}<div class="sub">${esc(expiryText(row))}</div></td>
    <td>${row.renew ? `<code>${esc(row.renew)}</code>` : `<span class="sub">none given</span>`}</td>
    <td>${users}</td>
    <td><button class="btn" data-secret-edit="${esc(row.name)}">Edit</button></td>
  </tr>`;
}

function undeclaredRow(row) {
  const expiry = row.expires ? `${expiryText(row)}` : "no expires:";
  return `<tr>
    <td>${esc(row.scope)}</td>
    <td>${esc(row.agent)}</td>
    <td>${esc(row.provider)}</td>
    <td><code>${esc(row.source)}</code></td>
    <td>${tag("undeclared", "warn")}<div class="sub">${esc(expiry)}</div></td>
  </tr>`;
}

function changeRow(change) {
  return `<li><span class="sub">${esc(change.at)}</span> ${esc(change.message)}</li>`;
}

function renderCatalogue() {
  const env = state.environmentError ? null : state.environment;
  const rows = (env && env.secrets) || [];
  const body = $("secrets-catalogue");
  if (body) body.innerHTML = rows.map(catalogueRow).join("");
  const empty = $("noCatalogue");
  if (empty) empty.hidden = rows.length !== 0 || !!state.environmentError;
  for (const button of document.querySelectorAll("[data-secret-edit]")) {
    button.onclick = () => openSecretEditor(rows.find(r => r.name === button.dataset.secretEdit));
  }

  const undeclared = visibleUndeclared(env && env.undeclared, inScope);
  const ubody = $("secrets-undeclared");
  if (ubody) ubody.innerHTML = undeclared.map(undeclaredRow).join("");
  const usection = $("secrets-undeclared-section");
  if (usection) usection.hidden = undeclared.length === 0;

  const changes = (env && env.secret_changes) || [];
  const list = $("secrets-changes");
  if (list) list.innerHTML = changes.slice().reverse().map(changeRow).join("");
  const csection = $("secrets-changes-section");
  if (csection) csection.hidden = changes.length === 0;
}

/// The one write: a declared secret's expiry, renew line and note. Nothing
/// here can carry a value.
async function openSecretEditor(row) {
  if (!row) return;
  // Loaded when first used: the module wires a document-wide key handler as
  // it loads, and this one stays importable where there is no document.
  const { closeModal, dropModal, scrim } = await import("./modal.js");
  const v = formValues(row);
  dropModal();
  scrim(`<header><div><h2>${esc(row.name)}</h2><code class="id">${esc(row.source)}</code></div>
      <button class="x" id="se-close">&times;</button></header>
    <div class="body">
      <p class="sub">Metadata only, written to the instance root's <code>secrets:</code> and journaled. The value stays where it is.</p>
      <label for="se-date">Expires</label>
      <input id="se-date" type="date" value="${esc(v.date)}" ${v.never ? "disabled" : ""}>
      <label class="checkrow"><input id="se-never" type="checkbox" ${v.never ? "checked" : ""}> never expires</label>
      <label for="se-renew">Renew <span class="sub" style="text-transform:none">one line, often a command</span></label>
      <input id="se-renew" value="${esc(v.renew)}">
      <label for="se-note">Note <span class="sub" style="text-transform:none">optional</span></label>
      <input id="se-note" value="${esc(v.note)}">
      <div class="err" id="se-err"></div>
      <div class="row-btns" style="margin-top:16px">
        <button class="btn primary" id="se-save">Save</button>
        <button class="btn" id="se-cancel">Cancel</button>
      </div>
    </div>`);
  $("se-close").onclick = closeModal;
  $("se-cancel").onclick = closeModal;
  $("se-never").onchange = () => { $("se-date").disabled = $("se-never").checked; };
  $("se-save").onclick = () => saveSecret(row.name, closeModal);
}

async function saveSecret(name, closeModal) {
  const form = {
    never: $("se-never").checked,
    date: $("se-date").value,
    renew: $("se-renew").value,
    note: $("se-note").value,
  };
  const err = $("se-err");
  if (!form.never && !validDate(form.date)) {
    err.textContent = "expires is a date (YYYY-MM-DD), or tick never";
    return;
  }
  err.textContent = "";
  $("se-save").disabled = true;
  try {
    const answer = await api(`/api/secrets/${encodeURIComponent(name)}`, {
      method: "PUT",
      body: JSON.stringify(metadataBody(form)),
    });
    closeModal();
    if (state.environment && Array.isArray(state.environment.secrets)) {
      state.environment.secrets = state.environment.secrets.map(r => (r.name === name ? answer.secret : r));
    }
    renderSecrets();
    if (refreshAfterSave) refreshAfterSave();
  } catch (e) {
    err.textContent = e.message;
    $("se-save").disabled = false;
  }
}

let refreshAfterSave = null;

/// What to call after a save, so the journal line and the rest of the page
/// come back from the daemon (app.js's `refreshEnvironment`).
export function onSecretSaved(fn) {
  refreshAfterSave = fn;
}

export function renderSecrets() {
  const correction = $("secrets-correction");
  if (correction) correction.textContent = CORRECTION;
  const boundary = $("secrets-boundary");
  if (boundary) boundary.textContent = BOUNDARY;

  const reach = $("secrets-reach");
  if (reach) reach.textContent = REACHABILITY;
  const failed = $("secrets-error");
  if (failed) {
    failed.textContent = state.environmentError || "";
    failed.hidden = !state.environmentError;
  }

  const body = $("secrets");
  if (!body) return;
  const rows = state.environmentError
    ? []
    : visibleCredentials(state.environment && state.environment.credentials);
  body.innerHTML = rows.map(credentialRow).join("");
  const empty = $("noSecrets");
  if (empty) empty.hidden = rows.length !== 0 || !!state.environmentError;
  renderCatalogue();
}
