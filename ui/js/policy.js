//! The L6 Direction, Policy tab (ADR 0004): a control catalogue checked
//! against evidence Factory already has -- never a policy engine (design
//! §8's deferral stands; see `.specs/adr/0004-policy-controls.md` in the
//! Business Factory root repository). `policy-model.js` holds every pure
//! shaping function this file draws from; this file only fetches, renders,
//! and wires clicks -- the same split `roles.js` keeps, for the same reason:
//! a Node test needs no DOM to exercise the shaping logic.
//!
//! Rail semantics: nothing selected fetches the whole instance
//! (`GET /api/policy`, whose `rollup` is the company-wide one); a scope
//! selected fetches `GET /api/policy?scope=<name>` (that scope and its
//! descendants) -- the daemon does the narrowing itself
//! (`Engine::policy_report`), the same pattern `roles.js`'s `loadRoles`
//! already follows for `GET /api/roles?scope=`.
//!
//! "Owner" actions -- Attest, Withdraw -- are shown unconditionally: the web
//! UI always runs as the owner (every agent shares the one socket; a person
//! at this page is never anyone else), the same reasoning `roles.js`'s own
//! header comment states for its guard-rail sentence.

import { $, api, esc, state } from "./core.js";
import { scrim, closeModal, dropModal } from "./modal.js";
import {
  attestBody,
  checkTarget,
  closingLinks,
  describeCheck,
  findingLabel,
  findingsByKind,
  frameworkCards,
  gapRows,
  kindLabel,
  notApplicableRows,
  refLinks,
  remediateAction,
  remediateBody,
  statusLabel,
  statusOf,
  taskHref,
} from "./policy-model.js";

/// Answers can arrive out of order when the rail moves quickly; only the
/// newest request is allowed to draw -- the same guard `roles.js`'s
/// `loadRoles` uses for the same reason.
let asked = 0;

/// The control detail modal currently open, `{scope, control}`, or `null`.
/// Not routed into the URL -- like `roles.js`'s edit form and
/// `datasets.js`'s "Add case"/"Run…", this is ephemeral UI state, not a
/// destination a link should be able to name; only the Tasks modal gets that
/// (`MODAL` in `app.js`).
let openControl = null;
let controlAsked = 0;

// ------------------------------------------------------------------ badges

function statusBadge(kind) {
  return `<span class="badge s-${esc(kind)}">${esc(statusLabel(kind))}</span>`;
}

function complianceBadge(compliant) {
  return `<span class="badge ${compliant ? "pol-compliant" : "pol-not-compliant"}">${compliant ? "compliant" : "not compliant"}</span>`;
}

function countsChips(counts) {
  return ["satisfied", "attested", "stale", "open", "not_applicable"]
    .map((k) => `<span class="pol-count s-${k}">${counts[k]} ${esc(statusLabel(k))}</span>`)
    .join("");
}

// -------------------------------------------------------------------- cards

function frameworkCard(card) {
  // A framework made only of best practice has nothing that counts, so it
  // is neither compliant nor not: its own counts are the whole card, and the
  // badge says why they do not decide anything.
  const onlyBestPractice = card.countedTotal === 0 && card.bestPracticeTotal > 0;
  return `<article class="pol-card">
    <div class="pol-card-head">
      <h3>${esc(card.title)}</h3>
      <code class="id">${esc(card.framework)}</code>
      <span class="sp"></span>
      ${onlyBestPractice ? `<span class="badge pol-not-counted">not counted</span>` : complianceBadge(card.compliant)}
    </div>
    ${card.kind ? `<div class="sub">${esc(kindLabel(card.kind))}</div>` : ""}
    <div class="pol-counts">${countsChips(onlyBestPractice ? card.bestPractice : card.counts)}</div>
    ${card.bestPracticeTotal && !onlyBestPractice
      ? `<div class="sub pol-bp">best practice (shown, never counted)</div>
         <div class="pol-counts">${countsChips(card.bestPractice)}</div>`
      : ""}
  </article>`;
}

// --------------------------------------------------------------------- gaps

function gapRow(row) {
  const links = closingLinks(row.scope, row.reasons);
  return `<tr>
    <td><button type="button" class="linklike" data-open-control="${esc(row.control)}" data-open-scope="${esc(row.scope)}">
      <div class="title">${esc(row.title)}</div><code class="id">${esc(row.control)}</code>
    </button></td>
    <td>${esc(row.scope)}</td>
    <td>${statusBadge(row.status)}</td>
    <td class="sub">${row.reasons.map(esc).join("; ")}</td>
    <td>
      <div class="pol-links">${links.length
        ? links.map((l) => `<a href="${esc(l.href)}">${esc(l.label)}</a>`).join("")
        : `<span class="sub">—</span>`}</div>
      ${remediateHtml(row.control, row.scope, remediateAction(row.status, row.openTask))}
    </td>
  </tr>`;
}

function naRow(row) {
  return `<tr>
    <td><div class="title">${esc(row.title)}</div><code class="id">${esc(row.control)}</code></td>
    <td>${esc(row.scope)}</td>
    <td class="sub">${esc(row.rationale)}</td>
  </tr>`;
}

function findingsHtml(findings) {
  const groups = findingsByKind(findings);
  return [...groups.entries()]
    .map(
      ([kind, rows]) => `
    <div class="pol-finding-group">
      <h4>${esc(findingLabel(kind))}</h4>
      <ul>${rows.map((f) => `<li><code>${esc(f.subject)}</code> — ${esc(f.detail)}</li>`).join("")}</ul>
    </div>`,
    )
    .join("");
}

// -------------------------------------------------------------- remediate
//
// One "Create task" button, in two places -- a gap row and the control-detail
// modal -- sharing one markup fragment and one click handler rather than two
// copies: both carry the same `data-remediate`/`data-remediate-scope` pair,
// and `wireRemediateButtons` is called once per place the markup lands
// (`renderPolicy` for the gap table, `wireControlBody` for the modal).
// Never `alert()`/`confirm()` -- a refusal (a control already satisfied, or
// a task already open) lands as inline text next to the button, the same
// restraint this page's Withdraw form already takes.
//
// When the report already names an open remediation task for the control
// (`open_tasks`/`open_task`, `#98`), the same cell holds a link to it
// instead of the button -- what a click would have been refused over. The
// daemon's refusal stays the guard for a task opened since the last load.

function remediateHtml(control, scope, action) {
  if (!action) return "";
  if (action.kind === "open") {
    return `<div class="pol-remediate" data-remediate-cell>
    <a class="pol-task-created" href="${esc(taskHref(scope, action.task))}">Task open →</a>
  </div>`;
  }
  return `<div class="pol-remediate" data-remediate-cell>
    <button type="button" class="btn" data-remediate="${esc(control)}" data-remediate-scope="${esc(scope)}">Create task</button>
  </div>`;
}

function wireRemediateButtons(root) {
  if (!root) return;
  for (const b of root.querySelectorAll("[data-remediate]")) {
    b.onclick = () => remediate(b);
  }
}

async function remediate(button) {
  const cell = button.closest("[data-remediate-cell]");
  const control = button.dataset.remediate;
  const scope = button.dataset.remediateScope;
  button.disabled = true;
  const existingError = cell.querySelector(".pol-remediate-err");
  if (existingError) existingError.remove();
  try {
    const answer = await api("/api/policy/remediate", {
      method: "POST",
      body: JSON.stringify(remediateBody(control, scope)),
    });
    const task = answer.task;
    cell.innerHTML = `<a class="pol-task-created" href="${esc(taskHref(scope, task.id))}" title="${esc(task.title)}">Task created →</a>`;
  } catch (e) {
    button.disabled = false;
    cell.insertAdjacentHTML("beforeend", `<div class="sub pol-remediate-err">${esc(e.message)}</div>`);
  }
}

// ------------------------------------------------------------------- board

export function renderPolicy() {
  const note = $("policy-scope-note");
  if (note) {
    note.textContent = state.scope === null
      ? "No scope selected: this is the whole instance's rollup, every configured scope folded in."
      : `${state.scope} and every scope below it.`;
  }
  const failed = $("policy-error");
  if (failed) {
    failed.textContent = state.policyError || "";
    failed.hidden = !state.policyError;
  }
  // Set unconditionally, before the failed-fetch early return below -- a
  // stale `href` from before a fetch failed would otherwise silently keep
  // pointing at the last scope that loaded rather than the one now
  // selected.
  const exportLink = $("policy-export");
  if (exportLink) {
    const query = state.scope === null ? "" : `?scope=${encodeURIComponent(state.scope)}`;
    exportLink.href = `/api/policy/export${query}`;
  }

  const report = state.policyError ? null : state.policy;
  const cardsEl = $("policy-cards");
  const countEl = $("policy-count");
  const emptyEl = $("policy-empty");
  const gapsBody = $("policy-gaps");
  const naBody = $("policy-na");
  const findingsEl = $("policy-findings");

  if (!report) {
    if (cardsEl) cardsEl.innerHTML = "";
    if (countEl) countEl.textContent = "";
    if (gapsBody) gapsBody.innerHTML = "";
    if (naBody) naBody.innerHTML = "";
    if (findingsEl) findingsEl.innerHTML = "";
    for (const id of ["policy-empty", "policy-no-gaps", "policy-no-na", "policy-no-findings"]) {
      const el = $(id);
      if (el) el.hidden = true;
    }
    return;
  }

  const cards = frameworkCards(report);
  if (countEl) countEl.textContent = `${cards.length} framework${cards.length === 1 ? "" : "s"}`;
  if (cardsEl) cardsEl.innerHTML = cards.map(frameworkCard).join("");
  if (emptyEl) emptyEl.hidden = cards.length !== 0;

  const gaps = gapRows(report);
  if (gapsBody) {
    gapsBody.innerHTML = gaps.map(gapRow).join("");
    for (const b of gapsBody.querySelectorAll("[data-open-control]")) {
      b.onclick = () => openControlDetail(b.dataset.openScope, b.dataset.openControl);
    }
    wireRemediateButtons(gapsBody);
  }
  const noGaps = $("policy-no-gaps");
  if (noGaps) noGaps.hidden = gaps.length !== 0;

  const na = notApplicableRows(report);
  if (naBody) naBody.innerHTML = na.map(naRow).join("");
  const noNa = $("policy-no-na");
  if (noNa) noNa.hidden = na.length !== 0;

  if (findingsEl) findingsEl.innerHTML = findingsHtml(report.findings);
  const noFindings = $("policy-no-findings");
  if (noFindings) noFindings.hidden = (report.findings || []).length !== 0;
}

export async function loadPolicy() {
  const mine = ++asked;
  const query = state.scope === null ? "" : `?scope=${encodeURIComponent(state.scope)}`;
  try {
    const answer = await api(`/api/policy${query}`);
    if (mine !== asked) return;
    state.policy = answer.report;
    state.policyError = null;
  } catch (e) {
    if (mine !== asked) return;
    state.policy = null;
    state.policyError = e.message;
  }
  renderPolicy();
}

/// `policy_changed` (an attestation recorded or withdrawn, anywhere in the
/// tree): the board's own rollup can change even when the control named is
/// not the one on screen -- an ancestor's attestation reaches every
/// descendant's evaluation too -- and the open control modal, if any, is
/// showing exactly the thing that just changed.
export function reloadPolicy() {
  loadPolicy();
  if (openControl && $("cd-body")) refreshControlDetail(controlAsked);
}

export function wirePolicy() {
  const button = $("policy-refresh");
  if (button) button.onclick = () => loadPolicy();
}

// ----------------------------------------------------------- control detail

function checksHtml(scope, control, checks) {
  return `<ul class="pol-checks">${(checks || [])
    .map((c) => {
      const target = checkTarget(scope, control, c);
      return `<li>${esc(describeCheck(c))}${target ? ` — <a href="${esc(target.href)}">${esc(target.label)}</a>` : ""}</li>`;
    })
    .join("")}</ul>`;
}

function when(iso) {
  return new Date(iso).toLocaleString();
}

function attestationRow(a) {
  const w = a.withdrawn;
  return `<tr class="${w ? "pol-withdrawn" : ""}">
    <td>${esc(a.attested_by)}</td>
    <td>${esc(a.evidence)}${a.note ? `<div class="sub">${esc(a.note)}</div>` : ""}</td>
    <td class="sub">${esc(when(a.attested_at))}</td>
    <td class="sub">${esc(when(a.expires_at))}</td>
    <td>${w
      ? `<span class="sub" title="${esc(w.reason || "")}">withdrawn by ${esc(w.by)}</span>`
      : `<button type="button" class="btn danger" data-withdraw="${esc(a.id)}">Withdraw</button>`}</td>
  </tr>`;
}

function attestFormHtml() {
  return `
    <h3 class="pol-sub-head">Attest</h3>
    <label for="cd-evidence">Evidence <span class="sub" style="text-transform:none">a pointer -- a document, a ticket, a page -- not the evidence itself</span></label>
    <input id="cd-evidence" placeholder="https://…">
    <label for="cd-expires">Expires <span class="sub" style="text-transform:none">30d, 12w, a date (2027-01-01), or a full timestamp</span></label>
    <input id="cd-expires" value="365d">
    <label for="cd-note">Note <span class="sub" style="text-transform:none">optional</span></label>
    <input id="cd-note">
    <div class="err" id="cd-err"></div>
    <div class="row-btns" style="margin-top:10px">
      <button class="btn primary" id="cd-attest">Record attestation</button>
    </div>`;
}

/// `refLinks`' own hrefs, direct from `detail.refs`' ids -- alongside the
/// reasons list, not folded into it: a reason is prose, a ref is a pointer,
/// and `refLinks` (unlike `closingLinks`, which has to parse a reason's own
/// text) never needs to.
function refsHtml(scope, refs) {
  const links = refLinks(scope, refs);
  if (!links.length) return "";
  return `<div class="pol-links">${links
    .map((l) => `<a href="${esc(l.href)}">${esc(l.label)} <code class="id">${esc(l.id)}</code></a>`)
    .join("")}</div>`;
}

function controlBodyHtml(detail) {
  const s = statusOf(detail);
  const na = detail.not_applicable;
  return `
    <div class="pol-detail-status">
      ${statusBadge(s.status)}
      ${detail.max_age ? `<span class="sub">max_age ${esc(detail.max_age)}</span>` : ""}
    </div>
    <ul class="pol-reasons">${s.reasons.map((r) => `<li>${esc(r)}</li>`).join("")}</ul>
    ${refsHtml(openControl.scope, detail.refs)}
    ${na ? `<p class="env-note warn">Not applicable at ${esc(na.scope)}: ${esc(na.rationale)}</p>` : ""}
    ${detail.maps_to && detail.maps_to.length
      ? `<p class="sub">maps_to: ${detail.maps_to.map(esc).join(", ")}</p>`
      : ""}
    ${detail.remediation ? `<p class="sub">${esc(detail.remediation.trim())}</p>` : ""}
    ${remediateHtml(detail.control, openControl.scope, remediateAction(s.status, detail.open_task))}

    <h3 class="pol-sub-head">Evidence</h3>
    ${checksHtml(openControl.scope, detail.control, detail.checks)}

    <h3 class="pol-sub-head">Attestations</h3>
    ${(detail.attestations || []).length
      ? `<table class="pol-attestations">
          <thead><tr><th>By</th><th>Evidence</th><th>Attested</th><th>Expires</th><th></th></tr></thead>
          <tbody>${detail.attestations.map(attestationRow).join("")}</tbody>
        </table>`
      : `<div class="empty">No attestations recorded.</div>`}

    ${na
      ? `<p class="sub">Not applicable here -- the server refuses an attestation for a control marked n/a.</p>`
      : attestFormHtml()}`;
}

function wireControlBody(detail) {
  const body = $("cd-body");
  if (!body) return;
  for (const b of body.querySelectorAll("[data-withdraw]")) {
    b.onclick = () => openWithdrawForm(b.dataset.withdraw);
  }
  const attestButton = $("cd-attest");
  if (attestButton) attestButton.onclick = () => submitAttest(detail.control);
  wireRemediateButtons(body);
}

async function refreshControlDetail(mine) {
  if (!openControl) return;
  const { scope, control } = openControl;
  const body = $("cd-body");
  if (!body) return; // the modal was closed while a fetch was in flight
  const [framework, id] = control.split("/");
  try {
    const answer = await api(
      `/api/policy/controls/${encodeURIComponent(framework)}/${encodeURIComponent(id)}?scope=${encodeURIComponent(scope)}`,
    );
    if (mine !== controlAsked || !openControl || !$("cd-body")) return;
    const title = $("cd-title");
    if (title) title.textContent = answer.detail.title;
    $("cd-body").innerHTML = controlBodyHtml(answer.detail);
    wireControlBody(answer.detail);
  } catch (e) {
    if (mine !== controlAsked || !openControl || !$("cd-body")) return;
    $("cd-body").innerHTML = `<div class="err">${esc(e.message)}</div>`;
  }
}

async function openControlDetail(scope, control) {
  dropModal();
  openControl = { scope, control };
  const mine = ++controlAsked;
  scrim(`
    <header><div><h2 id="cd-title">${esc(control)}</h2><code class="id">${esc(scope)}</code></div>
      <button class="x" id="cd-close">&times;</button></header>
    <div class="body" id="cd-body"><div class="empty">loading…</div></div>`);
  $("cd-close").onclick = () => { openControl = null; closeModal(); };
  await refreshControlDetail(mine);
}

async function submitAttest(control) {
  if (!openControl) return;
  const err = $("cd-err");
  if (err) err.textContent = "";
  const button = $("cd-attest");
  try {
    const body = attestBody(control, openControl.scope, {
      evidence: $("cd-evidence").value,
      expires: $("cd-expires").value,
      note: $("cd-note").value,
    });
    if (button) button.disabled = true;
    await api("/api/policy/attestations", { method: "POST", body: JSON.stringify(body) });
    await refreshControlDetail(controlAsked);
    await loadPolicy();
  } catch (e) {
    if (err) err.textContent = e.message;
    const b = $("cd-attest");
    if (b) b.disabled = false;
  }
}

/// A reason textarea and a confirm button, standing in for `confirm()` --
/// this page never uses it or `alert()`, even though a couple of older
/// views still do; every error here lands in `#wd-err` instead. Replaces the
/// control detail modal rather than stacking over it (`dropModal()` first,
/// the same rule every other form in this app follows), and "Cancel"/the
/// close button both return to it rather than leaving the page with nothing
/// open.
function openWithdrawForm(id) {
  if (!openControl) return;
  const { scope, control } = openControl;
  dropModal();
  scrim(`
    <header><div><h2>Withdraw attestation</h2><code class="id">${esc(id)}</code></div>
      <button class="x" id="wd-close">&times;</button></header>
    <div class="body">
      <p class="env-note">Append-only, like the rest of the store: this writes a new row that references
        the attestation above and never touches it -- the audit trail keeps both.</p>
      <label for="wd-reason">Reason <span class="sub" style="text-transform:none">optional</span></label>
      <textarea id="wd-reason" rows="3"></textarea>
      <div class="err" id="wd-err"></div>
      <div class="row-btns" style="margin-top:16px">
        <button class="btn danger" id="wd-confirm">Withdraw</button>
        <button class="btn" id="wd-cancel">Cancel</button>
      </div>
    </div>`);
  const back = () => openControlDetail(scope, control);
  $("wd-close").onclick = back;
  $("wd-cancel").onclick = back;
  $("wd-confirm").onclick = async () => {
    const err = $("wd-err");
    if (err) err.textContent = "";
    const button = $("wd-confirm");
    if (button) button.disabled = true;
    const reason = $("wd-reason").value.trim();
    const query = reason ? `?reason=${encodeURIComponent(reason)}` : "";
    try {
      await api(`/api/policy/attestations/${encodeURIComponent(id)}/withdraw${query}`, { method: "POST" });
      await loadPolicy();
      await back();
    } catch (e) {
      if (err) err.textContent = e.message;
      if (button) button.disabled = false;
    }
  };
}
