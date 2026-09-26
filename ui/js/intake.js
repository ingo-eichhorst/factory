//! The L4 Process Intake view (`#119`): the inbound quality gate, read from
//! `GET /api/intake` -- the same board `factory intake` prints. Four columns
//! (received, triaging, needs info, ready), each card with the seven
//! readiness axes as a row of marks, and category, priority and estimate
//! once it is assessed. `intake-model.js` holds every pure function; this
//! file fetches, draws and wires clicks.
//!
//! Rail semantics: one read, narrowed by the daemon to the selected scope
//! and its subtree, the reading Operations uses.
//!
//! Every action is a dialog in the app's own modal and goes to the daemon,
//! which journals it with who asked. Nothing here reaches outside Factory:
//! no comment, label or reply is posted anywhere.

import { $, api, esc, state } from "./core.js";
import { scrim, closeModal, dropModal } from "./modal.js";
import { openTask } from "./tasks.js";
import {
  ACTION_LABELS,
  COLUMNS,
  COSTS,
  LEVELS,
  WONTFIX_REASONS,
  addRequest,
  agentText,
  assessRequest,
  assessmentProblem,
  axisMarks,
  buildAssessment,
  buildParts,
  cardActions,
  cardNote,
  cards,
  decideProblem,
  decideRequest,
  emptyPart,
  estimateOf,
  estimateText,
  fmtAge,
  infoRequest,
  nextActions,
  previewVerdict,
  priorityOf,
  routeFor,
  routeProblem,
  splitDraft,
  splitProblem,
  totalOpen,
  triageRequest,
  verdictChips,
  workflowIn,
} from "./intake-model.js";

let board = null;
let boardError = null;
let asked = 0;
let visible = false;

export async function loadIntake() {
  const mine = ++asked;
  const scope = state.scope;
  let answer = null;
  let error = null;
  try {
    answer = await api(`/api/intake${scope === null ? "" : `?scope=${encodeURIComponent(scope)}`}`);
  } catch (e) { error = e.message; }
  if (mine !== asked) return;
  board = answer ? answer.board : null;
  boardError = error;
  renderIntake();
}

export function showIntake() { visible = true; loadIntake(); }
export function hideIntake() { visible = false; }

/// A refetch for an event that touched an item, only while the view is on
/// screen -- `touchesIntake` in the model says which events those are.
export function refreshIntake() { if (visible) loadIntake(); }

export function wireIntake() {
  const refresh = $("intake-refresh");
  if (refresh) refresh.onclick = () => loadIntake();
  const add = $("intake-add");
  if (add) add.onclick = () => openAddDialog();
}

// ---------------------------------------------------------------- drawing

const MARK_TEXT = { pass: "✓", fail: "✗", tolerated: "~", unassessed: "·" };

function marksRow(card, axes) {
  return axisMarks(card, axes)
    .map(m => `<span class="ik-axis ik-${m.mark}" title="${esc(`${m.label}: ${m.evidence || m.pass_condition}`)}">${MARK_TEXT[m.mark]}</span>`)
    .join("");
}

function chips(card) {
  const v = verdictChips(card);
  if (!v) return "";
  return `<div class="ik-chips"><span class="badge ik-p ik-${esc(v.priority)}">${esc(v.priority)}</span>
    <span class="tag">${esc(v.category)}</span><span class="sub">${esc(v.estimate)}</span></div>`;
}

/// One card. `axes` is the board's own list, so a card never keeps a copy.
export function intakeCard(card, axes = board ? board.axes : []) {
  const note = cardNote(card);
  return `
    <div class="kbc ik-card" data-id="${esc(card.id)}" tabindex="0" role="button">
      <div class="kbc-top"><span class="ik-marks" aria-label="readiness axes">${marksRow(card, axes)}</span>
        <span class="sub" title="waiting since ${esc(card.received_at)}">${esc(fmtAge(card.age_seconds))}</span></div>
      <div class="title">${esc(card.title)}</div>
      <div class="sub">${esc(card.scope)} · from ${esc(card.requester)} · ${esc(card.source.kind)}</div>
      ${chips(card)}
      ${card.questions && card.questions.length ? `<div class="sub ik-q">? ${esc(card.questions[0])}${card.questions.length > 1 ? ` (+${card.questions.length - 1})` : ""}</div>` : ""}
      ${note ? `<div class="sub ik-note">${esc(note)}</div>` : ""}
    </div>`;
}

export function renderIntake() {
  const host = $("intake");
  if (!host) return;
  const err = $("intake-error");
  if (err) { err.hidden = !boardError; err.textContent = boardError || ""; }
  if (!board) { host.innerHTML = boardError ? "" : "loading…"; return; }
  const sub = $("intake-summary");
  if (sub) {
    sub.textContent = `${totalOpen(board)} open · ${cards(board, "ready").length} released, ${board.split || 0} split and ${board.wontfix} closed in the last ${board.ready_window_days} days`;
  }
  host.innerHTML = `<div class="kbcols">${COLUMNS.map(c => {
    const items = cards(board, c.key);
    return `
      <div class="kbcol" data-col="ik-${c.key}">
        <div class="kbcol-head" title="${esc(c.hint)}"><span>${esc(c.label)}</span><span class="kbcol-count">${items.length}</span></div>
        <div class="kbcol-body">${items.length ? items.map(c => intakeCard(c, board.axes)).join("") : `<div class="kbcol-empty">—</div>`}</div>
      </div>`;
  }).join("")}</div>`;
  for (const el of host.querySelectorAll(".ik-card")) {
    const open = () => openItem(el.dataset.id);
    el.onclick = open;
    el.onkeydown = (e) => { if (e.key === "Enter") open(); };
  }
}

function findCard(id) {
  if (!board) return null;
  for (const c of COLUMNS) {
    const found = cards(board, c.key).find(x => x.id === id);
    if (found) return found;
  }
  return null;
}

// ----------------------------------------------------------------- dialogs

/// The route as a sentence: scope, agent or workflow, its inputs and the
/// steps given to another agent.
function routeText(routing) {
  let out = routing.scope;
  if (routing.agent) out += ` as ${routing.agent}`;
  if (routing.workflow) {
    const w = workflowIn(routeFor(board, routing.scope), routing.workflow);
    out += `, workflow ${w ? w.name : routing.workflow}`;
    const inputs = Object.entries(routing.inputs || {}).map(([k, v]) => `${k}=${v}`);
    if (inputs.length) out += ` with ${inputs.join(", ")}`;
    const agents = Object.entries(routing.agents || {}).map(([k, v]) => `${k} by ${v}`);
    if (agents.length) out += `; ${agents.join(", ")}`;
  }
  return out;
}

/// What would move a held-back item: one block per action, its reasons,
/// and the button that does it -- so a red item is never a dead end.
function nextActionsBlock(card) {
  const next = nextActions(card);
  if (!next.length) return "";
  return `<div class="ik-next"><label>What moves it forward</label>${next.map(n => `
    <div class="ik-next-item"><p>${esc(n.hint)}</p>
      <ul class="sub">${n.reasons.map(r => `<li>${esc(r)}</li>`).join("")}</ul>
      <button class="btn primary" data-act="${n.act}">${esc(n.label)}</button></div>`).join("")}</div>`;
}

function proposedSplit(t) {
  const parts = (t && t.assessment.split) || [];
  if (!parts.length) return "";
  return `<label>Proposed split</label><ol class="ik-parts">${parts.map(p => `
    <li><b>${esc(p.title)}</b> <code>${esc(p.id)}</code>${(p.depends_on || []).length ? ` <span class="sub">after ${esc(p.depends_on.join(", "))}</span>` : ""}</li>`).join("")}</ol>`;
}

/// One item: its axes with evidence, what was asked, and what can be done.
export function openItem(id) {
  const card = findCard(id);
  if (!card) return;
  dropModal();
  const marks = axisMarks(card, board.axes);
  const t = card.triage;
  const next = nextActions(card).map(n => n.act);
  const actions = cardActions(card).filter(a => !next.includes(a));
  scrim(`
    <header><div><h2>${esc(card.title)}</h2><code class="id">${esc(card.id)}</code></div>
      <button class="x" id="ik-close" aria-label="Close">&times;</button></header>
    <div class="body">
      <p class="sub">${esc(card.stage.replace("_", " "))} · ${esc(card.scope)} · from ${esc(card.requester)} (${esc(card.source.kind)}${card.source.reference ? `: ${esc(card.source.reference)}` : ""}) · waiting ${esc(fmtAge(card.age_seconds))}</p>
      ${t ? `<p><span class="badge ik-p ik-${esc(t.priority)}">${esc(t.priority)}</span> <span class="tag">${esc(t.assessment.category)}</span>
        impact ${esc(t.assessment.impact)} × urgency ${esc(t.assessment.urgency)} · complexity ${esc(t.assessment.complexity)} · ${esc(estimateText(t.estimate))}
        · route ${esc(routeText(t.assessment.routing))}
        <br><span class="sub">assessed by ${esc(t.by)}: ${esc(t.verdict.verdict.replace("_", "-"))}</span></p>
        ${t.assessment.summary ? `<p>${esc(t.assessment.summary)}</p>` : ""}` : `<p class="sub">Not assessed yet.</p>`}
      <table class="ik-axes"><tbody>${marks.map(m => `
        <tr class="ik-${m.mark}"><td>${MARK_TEXT[m.mark]}</td><th scope="row">${esc(m.label)}</th>
          <td>${m.evidence ? esc(m.evidence) : `<span class="sub">${esc(m.pass_condition)}</span>`}${m.cost ? ` <span class="sub">(cost ${esc(m.cost)})</span>` : ""}</td></tr>`).join("")}
      </tbody></table>
      ${card.questions && card.questions.length ? `<label>Asked of the requester</label><ul>${card.questions.map(q => `<li>${esc(q)}</li>`).join("")}</ul>` : ""}
      ${proposedSplit(t)}
      ${nextActionsBlock(card)}
      ${card.decision ? `<p class="sub">decided ${esc(card.decision.decision.decision.replace("_", "-"))} by ${esc(card.decision.by)}</p>` : ""}
      <div class="row-btns" style="margin-top:16px">
        ${actions.map(a => `<button class="btn ${a === "release" ? "primary" : a === "wontfix" ? "danger" : ""}" data-act="${a}">${esc(ACTION_LABELS[a])}</button>`).join("")}
        <button class="btn" id="ik-task">Open as task</button>
      </div>
    </div>`);
  $("ik-close").onclick = closeModal;
  $("ik-task").onclick = () => openTask(card.id);
  for (const b of document.querySelectorAll(".scrim [data-act]")) {
    b.onclick = () => openAction(b.dataset.act, card);
  }
}

function openAction(action, card) {
  if (action === "assess") return openAssessDialog(card);
  if (action === "triage") return openTriageDialog(card);
  if (action === "info") return openInfoDialog(card);
  if (action === "split") return openSplitDialog(card);
  return openDecideDialog(action, card);
}

/// A dialog whose single button sends one request; a refusal stays in the
/// dialog as the daemon's own sentence.
function dialog(title, subtitle, body, button, build, problem = () => null, danger = false) {
  dropModal();
  scrim(`
    <header><div><h2>${esc(title)}</h2>${subtitle ? `<code class="id">${esc(subtitle)}</code>` : ""}</div>
      <button class="x" id="ik-close" aria-label="Close">&times;</button></header>
    <div class="body">${body}
      <div class="err" id="ik-err" role="alert"></div>
      <div class="row-btns" style="margin-top:16px">
        <button class="btn ${danger ? "danger" : "primary"}" id="ik-go">${esc(button)}</button>
        <button class="btn" id="ik-back">Back</button>
      </div>
    </div>`);
  $("ik-close").onclick = closeModal;
  $("ik-back").onclick = closeModal;
  const go = $("ik-go");
  let inFlight = false;
  const sync = () => {
    const why = problem();
    go.disabled = inFlight || !!why;
    go.title = why || "";
  };
  for (const el of document.querySelectorAll(".scrim input, .scrim textarea, .scrim select")) {
    el.oninput = sync; el.onchange = sync;
  }
  sync();
  go.onclick = async () => {
    if (inFlight) return;
    const req = build();
    if (!req) return;
    inFlight = true;
    sync();
    $("ik-err").textContent = "";
    try {
      await api(req.path, { method: req.method, body: JSON.stringify(req.body) });
      // A follow-up, such as a triage run after information: the first
      // request stands either way, so a refusal here is only reported.
      if (req.after) {
        try {
          await api(req.after.path, { method: req.after.method, body: JSON.stringify(req.after.body) });
        } catch (e) {
          $("ik-err").textContent = `saved, but: ${e.message}`;
          loadIntake();
          return;
        }
      }
      closeModal();
      loadIntake();
    } catch (e) {
      $("ik-err").textContent = e.message;
      inFlight = false;
      sync();
    }
  };
  return sync;
}

const scopeOptions = (selected) =>
  (state.scopeNames || []).map(n => `<option value="${esc(n)}"${n === selected ? " selected" : ""}>${esc(n)}</option>`).join("");

function openAddDialog() {
  dialog("Hand in work", "intake", `
    <p class="env-note">It is held in intake, and no agent runs it, until it is triaged and released.</p>
    <label for="ik-title">Title</label><input id="ik-title">
    <label for="ik-text">What is asked</label><textarea id="ik-text" rows="5"></textarea>
    <label for="ik-scope">Scope <span class="sub">where it is thought to belong; triage routes it</span></label>
    <select id="ik-scope">${scopeOptions(state.scope || (state.scopeNames || [])[0])}</select>
    <label for="ik-requester">On behalf of <span class="sub">optional</span></label><input id="ik-requester">
    <label for="ik-ref">Reference <span class="sub">optional: an issue URL, a mail id</span></label><input id="ik-ref">`,
  "Hand in",
  () => addRequest({ title: $("ik-title").value, instructions: $("ik-text").value, scope: $("ik-scope").value,
    requester: $("ik-requester").value, reference: $("ik-ref").value }),
  () => ($("ik-title") && $("ik-title").value.trim() ? null : "give it a title"));
  $("ik-title").focus();
}

function openTriageDialog(card) {
  dialog("Triage with an agent", card.title, `
    <p class="env-note">Starts a run that reads the item, scores the seven readiness axes and submits an
      assessment with its decision. It changes no file and posts nothing outside Factory.</p>
    <label for="ik-agent">Agent <span class="sub">optional: the scope's own when empty</span></label><input id="ik-agent">`,
  "Start triage",
  () => triageRequest(card.id, $("ik-agent").value));
}

function openInfoDialog(card) {
  dialog("Add information", card.title, `
    ${card.questions && card.questions.length ? `<label>What was asked</label><ul>${card.questions.map(q => `<li>${esc(q)}</li>`).join("")}</ul>` : ""}
    <label for="ik-info">The answer</label><textarea id="ik-info" rows="5"></textarea>
    <label class="checkrow"><input type="checkbox" id="ik-retriage" checked><span>Triage it again straight away</span></label>`,
  "Add and requeue",
  () => infoRequest(card.id, $("ik-info").value, $("ik-retriage").checked),
  () => ($("ik-info") && $("ik-info").value.trim() ? null : "write the information"));
}

/// Split an item into smaller ones, each handed back into intake. Opens on
/// the assessment's proposal, or two empty parts to write.
function openSplitDialog(card) {
  let rows = splitDraft(card);
  const read = () => [...document.querySelectorAll(".scrim [data-part]")].map(el => ({
    id: el.querySelector("[data-f=id]").value,
    title: el.querySelector("[data-f=title]").value,
    instructions: el.querySelector("[data-f=instructions]").value,
    depends_on: el.querySelector("[data-f=depends_on]").value,
    acceptance: el.querySelector("[data-f=acceptance]").value,
  }));
  const partsHtml = () => rows.map((r, i) => `
    <fieldset class="ik-part" data-part="${i}"><legend>Part ${i + 1}</legend>
      <div class="ik-grid">
        <label>Id <input data-f="id" value="${esc(r.id)}"></label>
        <label>Comes after <input data-f="depends_on" value="${esc(r.depends_on)}" placeholder="ids, comma-separated"></label>
      </div>
      <label>Title <input data-f="title" value="${esc(r.title)}"></label>
      <label>What it covers <textarea data-f="instructions" rows="3">${esc(r.instructions)}</textarea></label>
      <label>Done when <input data-f="acceptance" value="${esc(r.acceptance)}" placeholder="a command or a sentence"></label>
      ${rows.length > 2 ? `<button class="btn" data-remove="${i}">Remove part</button>` : ""}
    </fieldset>`).join("");
  const proposed = card.triage && (card.triage.assessment.split || []).length;
  const sync = dialog("Split into items", card.title, `
    <p class="env-note">${proposed ? "The assessment's proposal, to edit." : "No split was proposed: write the parts, or triage it again for a proposal."}
      Each part becomes an intake item of its own in ${esc(card.scope)}, carrying this item's text for context,
      and is triaged on its own. This item closes as split.</p>
    <div id="ik-parts">${partsHtml()}</div>
    <button class="btn" id="ik-add-part">Add part</button>`,
  "Split",
  () => decideRequest(card.id, "split", { parts: buildParts(read()) }),
  () => splitProblem(buildParts(read())));
  const redraw = () => {
    $("ik-parts").innerHTML = partsHtml();
    wireParts();
    sync();
  };
  const wireParts = () => {
    for (const el of document.querySelectorAll(".scrim #ik-parts input, .scrim #ik-parts textarea")) el.oninput = sync;
    for (const b of document.querySelectorAll(".scrim [data-remove]")) {
      b.onclick = () => { rows = read(); rows.splice(Number(b.dataset.remove), 1); redraw(); };
    }
  };
  $("ik-add-part").onclick = () => { rows = read(); rows.push(emptyPart(rows.length + 1)); redraw(); };
  wireParts();
}

function openDecideDialog(action, card) {
  const values = () => ({
    run: $("ik-run") ? $("ik-run").checked : false,
    questions: $("ik-questions") ? $("ik-questions").value : "",
    reason: $("ik-reason") ? $("ik-reason").value : "",
    evidence: $("ik-evidence") ? $("ik-evidence").value : "",
    duplicate_of: $("ik-dup") ? $("ik-dup").value : "",
  });
  let body = "";
  if (action === "release") {
    const t = card.triage;
    body = `<p class="env-note">Releases it into ${esc(t.assessment.routing.workflow ? `workflow ${t.assessment.routing.workflow}` : t.assessment.routing.scope)}
      as ${esc(t.assessment.category)}, ${esc(t.priority)}, ${esc(estimateText(t.estimate))}. The triage verdict is journaled as its first attestation.</p>
      ${t.assessment.routing.workflow ? "" : `<label class="checkrow"><input type="checkbox" id="ik-run"><span>Run it now</span></label>`}`;
  } else if (action === "needs_info") {
    const suggested = (card.triage && card.triage.assessment.questions) || [];
    body = `<p class="env-note">Sends it back to ${esc(card.requester)}. It stays in intake until information comes.</p>
      <label for="ik-questions">Questions <span class="sub">one per line; empty asks the assessment's, or its failed axes</span></label>
      <textarea id="ik-questions" rows="4">${esc(suggested.join("\n"))}</textarea>`;
  } else {
    body = `<p class="env-note">Closes it. Only for a verified duplicate, an invalid report or something that is not ours to do.</p>
      <label for="ik-reason">Reason</label><select id="ik-reason"><option value="">…</option>${WONTFIX_REASONS.map(r => `<option value="${r.key}">${esc(r.label)}</option>`).join("")}</select>
      <label for="ik-evidence">Evidence <span class="sub">what verifies it</span></label><textarea id="ik-evidence" rows="3"></textarea>
      <label for="ik-dup">Duplicate of <span class="sub">for a duplicate: a task id or an issue URL</span></label><input id="ik-dup">`;
  }
  dialog(ACTION_LABELS[action], card.title, body, ACTION_LABELS[action],
    () => decideRequest(card.id, action, values()),
    () => decideProblem(action, values()),
    action === "wontfix");
}

function openAssessDialog(card) {
  const prior = card.triage ? card.triage.assessment : null;
  const priorAxis = (axis) => (prior ? prior.axes.find(a => a.axis === axis) : null) || { pass: true, evidence: "" };
  const opts = (list, selected) => list.map(v => `<option${v === selected ? " selected" : ""}>${v}</option>`).join("");
  const rows = board.axes.map(({ axis, label, pass_condition }) => {
    const p = priorAxis(axis);
    return `<tr><th scope="row"><label class="checkrow"><input type="checkbox" data-axis="${axis}"${p.pass ? " checked" : ""}><span>${esc(label)}</span></label></th>
      <td><input data-evidence="${axis}" placeholder="${esc(pass_condition)}" value="${esc(p.evidence)}">
      ${axis === "observability" ? `<select data-cost title="cost, when it fails"><option value="">cost…</option>${opts(COSTS, p.cost)}</select>` : ""}</td></tr>`;
  }).join("");
  const route = prior ? prior.routing : { scope: card.scope };
  const fieldValues = (attr) => {
    const out = {};
    for (const el of document.querySelectorAll(`.scrim [${attr}]`)) out[el.getAttribute(attr)] = el.value;
    return out;
  };
  const values = () => ({
    axes: board.axes.map(({ axis }) => ({
      axis,
      pass: document.querySelector(`.scrim [data-axis="${axis}"]`).checked,
      evidence: document.querySelector(`.scrim [data-evidence="${axis}"]`).value,
      cost: axis === "observability" ? document.querySelector(".scrim [data-cost]").value : null,
    })),
    category: $("ik-category").value,
    impact: $("ik-impact").value,
    urgency: $("ik-urgency").value,
    complexity: $("ik-complexity").value,
    scope: $("ik-route").value,
    agent: $("ik-route-agent") ? $("ik-route-agent").value : "",
    workflow: $("ik-route-workflow") ? $("ik-route-workflow").value : "",
    inputs: fieldValues("data-input"),
    agents: fieldValues("data-step"),
    // A proposal made earlier stays with a hand re-assessment.
    split: prior && (prior.split || []).length ? splitDraft(card) : [],
    summary: $("ik-summary").value,
    questions: $("ik-ask").value,
  });
  const sync = dialog("Assess", card.title, `
    <p class="env-note">Each axis passes or fails with one sentence of evidence. Any failure but a low- or
      medium-cost observability gap makes it needs-info; complexity 9-10 too.</p>
    <table class="ik-axes ik-form"><tbody>${rows}</tbody></table>
    <label for="ik-category">Category</label>
    <input id="ik-category" list="ik-categories" value="${esc(prior ? prior.category : "")}">
    <datalist id="ik-categories">${(board.categories || []).map(c => `<option value="${esc(c)}">`).join("")}</datalist>
    <div class="ik-grid">
      <label>Impact <select id="ik-impact">${opts(LEVELS, prior ? prior.impact : "medium")}</select></label>
      <label>Urgency <select id="ik-urgency">${opts(LEVELS, prior ? prior.urgency : "medium")}</select></label>
      <label>Complexity <input id="ik-complexity" type="number" min="1" max="10" value="${esc(prior ? prior.complexity : 3)}"></label>
    </div>
    <label for="ik-route">Route to</label><select id="ik-route">${scopeOptions(route.scope)}</select>
    <div id="ik-route-how"></div>
    <label for="ik-summary">Summary</label><textarea id="ik-summary" rows="2">${esc(prior ? prior.summary : "")}</textarea>
    <label for="ik-ask">Questions for a needs-info <span class="sub">one per line</span></label>
    <textarea id="ik-ask" rows="2">${esc(prior ? (prior.questions || []).join("\n") : "")}</textarea>
    <p class="ik-preview" id="ik-preview" aria-live="polite"></p>
    <label class="checkrow"><input type="checkbox" id="ik-decide" checked><span>Apply the verdict: release if ready, send back if needs-info</span></label>`,
  "Record assessment",
  () => assessRequest(card.id, buildAssessment(values()), $("ik-decide").checked),
  () => {
    const a = buildAssessment(values());
    const v = previewVerdict(a);
    const pv = $("ik-preview");
    if (pv) {
      pv.textContent = `${v.verdict === "ready" ? "ready" : `needs-info (${v.blockers.join(", ")})`} · ${priorityOf(a.impact, a.urgency) || "-"} · ${estimateText(estimateOf(a.complexity))}`;
    }
    return assessmentProblem(a) || routeProblem(a, board);
  });
  // How it runs there: an agent, or a workflow with its inputs and an
  // agent -- and so a model -- per step. Redrawn when the scope or the
  // workflow changes, keeping what was chosen where it still applies.
  const drawRoute = (keep) => {
    const r = routeFor(board, $("ik-route").value);
    const w = workflowIn(r, keep.workflow);
    const agentOptions = (selected, none) => `<option value="">${esc(none)}</option>${(r.agents || []).map(a =>
      `<option value="${esc(a.name)}"${a.name === selected ? " selected" : ""}>${esc(agentText(a))}</option>`).join("")}`;
    $("ik-route-how").innerHTML = `
      <label for="ik-route-workflow">Workflow <span class="sub">the way of working that suits it best</span></label>
      <select id="ik-route-workflow"><option value="">none: run it as one task</option>${(r.workflows || []).map(x =>
        `<option value="${esc(x.id)}"${w && w.id === x.id ? " selected" : ""}>${esc(x.name)}${x.description ? ` -- ${esc(x.description)}` : ""}</option>`).join("")}</select>
      ${w ? `${(w.inputs || []).map(i => `
        <label>Input ${esc(i.name)} <span class="sub">${esc(i.description || "")}</span>
          <input data-input="${esc(i.name)}" value="${esc((keep.inputs || {})[i.name] || "")}"></label>`).join("")}
        <table class="ik-axes ik-form"><tbody>${(w.steps || []).map(s => `
          <tr><th scope="row">${esc(s.id)}</th><td class="sub">${esc(s.title)}</td>
            <td><select data-step="${esc(s.id)}" title="agent, and so model, for this step">${agentOptions((keep.agents || {})[s.id], `as defined${s.agent ? `: ${s.agent}` : ""}`)}</select></td></tr>`).join("")}
        </tbody></table>`
      : `<label for="ik-route-agent">Agent</label><select id="ik-route-agent">${agentOptions(keep.agent, `the scope's own${r.default_agent ? `: ${r.default_agent}` : ""}`)}</select>`}`;
    for (const el of document.querySelectorAll(".scrim #ik-route-how input, .scrim #ik-route-how select")) {
      el.oninput = sync; el.onchange = sync;
    }
    $("ik-route-workflow").onchange = () => {
      drawRoute({ ...currentRoute(), workflow: $("ik-route-workflow").value });
      sync();
    };
  };
  const currentRoute = () => ({
    workflow: $("ik-route-workflow") ? $("ik-route-workflow").value : "",
    agent: $("ik-route-agent") ? $("ik-route-agent").value : "",
    inputs: fieldValues("data-input"),
    agents: fieldValues("data-step"),
  });
  $("ik-route").onchange = () => { drawRoute({ ...currentRoute(), workflow: "" }); sync(); };
  drawRoute(route);
  sync();
}
