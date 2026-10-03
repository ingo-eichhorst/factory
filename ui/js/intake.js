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
//! which journals it with who asked. Every request here still only ever
//! reaches Factory's own API -- the one outward effect, approving a decided
//! GitHub item's comment and labels (`#171`), is the daemon's `gh` call to
//! make once asked; nothing here talks to GitHub directly.

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
  basisText,
  decideProblem,
  decideRequest,
  duplicateRows,
  emptyPart,
  estimateOf,
  estimateText,
  fmtAge,
  flagSecurityRequest,
  hasConfirmedSecurityReport,
  infoRequest,
  nextActions,
  outboundInfo,
  OUTBOUND_STATE_LABELS,
  previewVerdict,
  priorityOf,
  publishRequest,
  routeFor,
  routeProblem,
  securityDecisionProblem,
  securityDecisionRequest,
  securityFlag,
  sourceText,
  splitDraft,
  splitProblem,
  totalOpen,
  triageRequest,
  verdictChips,
  wontfixDraft,
  workflowIn,
} from "./intake-model.js";
import { clockIndex, nowFromClock, reportDeadlines } from "./clock-model.js";

let board = null;
let boardError = null;
let asked = 0;
let visible = false;

/// The reporting clock (`#157`/`#170` phase 2), read alongside the board
/// only when it might say something -- `hasConfirmedSecurityReport`, below.
/// `null` covers both "not read" and "the read failed": either way, the
/// card and modal fall back to showing no deadlines, never a broken page --
/// `clockIndex(null)` is an empty map, so `reportDeadlines` simply finds
/// nothing.
let clock = null;
let clockIdx = clockIndex(null);

export async function loadIntake() {
  const mine = ++asked;
  const scope = state.scope;
  const query = scope === null ? "" : `?scope=${encodeURIComponent(scope)}`;
  let answer = null;
  let error = null;
  try {
    answer = await api(`/api/intake${query}`);
  } catch (e) { error = e.message; }
  if (mine !== asked) return;
  board = answer ? answer.board : null;
  boardError = error;
  if (board && hasConfirmedSecurityReport(board)) {
    try {
      clock = (await api(`/api/policy/clock${query}`)).clock;
    } catch {
      clock = null;
    }
    if (mine !== asked) return;
  } else {
    clock = null;
  }
  clockIdx = clockIndex(clock);
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
    <span class="tag">${esc(v.category)}</span><span class="sub" title="${v.basis ? esc(v.basis) : ""}">${esc(v.estimate)}</span></div>`;
}

/// A security flag's badge (`#170`): the fast-lane band `board()` already
/// sorted into place, shown so a card in it is never a mystery. `null` for
/// an item nobody has ever flagged.
function securityBadge(card) {
  const flag = securityFlag(card);
  if (!flag) return "";
  const label = { possible: "possible security report", confirmed: "confirmed security report", dismissed: "security report dismissed" }[flag.state] || flag.state;
  return `<span class="badge ik-sec ik-sec-${esc(flag.state)}" title="${esc(flag.reason || "")}">${esc(label)}</span>`;
}

/// A confirmed report's own CRA Art. 14 deadlines (`#157`/`#170` phase 2),
/// one badge per deadline -- empty when the clock was not read (no confirmed
/// report on this board) or has nothing for this card yet (`reportDeadlines`
/// resolves the root through server-derived membership). The final report appears
/// when corrective-measure evidence supplies its anchor.
function clockDeadlinesHtml(card) {
  if (!clock) return "";
  const item = reportDeadlines(clockIdx, card, nowFromClock(clock));
  if (!item) return "";
  return item.deadlines
    .map(
      (d) =>
        `<span class="badge s-${esc(d.state)}" title="${esc(d.text)} · due ${esc(d.dueAt)}">${esc(d.label)}: ${esc(d.stateLabel)}</span>`,
    )
    .join("") + (item.awaitingMeasure ? `<span class="badge s-stale">final report: awaiting measure evidence</span>` : "");
}

/// A decided GitHub item's outbound state (`#171`), shown wherever a card or
/// the item modal says what is happening to it -- empty for anything not
/// from GitHub or with nothing decided yet.
function outboundNote(card) {
  const info = outboundInfo(card);
  if (!info) return "";
  return `GitHub: ${OUTBOUND_STATE_LABELS[info.state] || info.state}`;
}

/// One card. `axes` is the board's own list, so a card never keeps a copy.
export function intakeCard(card, axes = board ? board.axes : []) {
  const note = cardNote(card);
  const outbound = outboundNote(card);
  const badge = securityBadge(card);
  const deadlines = clockDeadlinesHtml(card);
  return `
    <div class="kbc ik-card${badge ? " ik-fast-lane" : ""}" data-id="${esc(card.id)}" tabindex="0" role="button">
      <div class="kbc-top"><span class="ik-marks" aria-label="readiness axes">${marksRow(card, axes)}</span>
        <span class="sub" title="waiting since ${esc(card.received_at)}">${esc(fmtAge(card.age_seconds))}</span></div>
      ${badge ? `<div class="ik-chips">${badge}${deadlines}</div>` : ""}
      <div class="title">${esc(card.title)}</div>
      <div class="sub">${esc(card.scope)} · from ${esc(card.requester)} · ${esc(sourceText(card.source))}</div>
      ${chips(card)}
      ${card.questions && card.questions.length ? `<div class="sub ik-q">? ${esc(card.questions[0])}${card.questions.length > 1 ? ` (+${card.questions.length - 1})` : ""}</div>` : ""}
      ${note ? `<div class="sub ik-note">${esc(note)}</div>` : ""}
      ${outbound ? `<div class="sub ik-note">${esc(outbound)}</div>` : ""}
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

/// The item modal's "Possible duplicates" block: what the daemon found,
/// overlaid with the triager's own answer (`duplicateRows`) -- kind,
/// reference, title, how it matched and its verdict. Empty when there is
/// nothing to show.
export function candidatesBlock(card) {
  const rows = duplicateRows(card);
  if (!rows.length) return "";
  return `<label>Possible duplicates</label><ul class="ik-dupes">${rows.map(r => `
    <li><span class="tag">${esc(r.kind)}</span> <code>${esc(r.reference)}</code> ${esc(r.title)}
      <span class="badge ik-dup-${esc(r.verdict)}">${esc(r.verdict)}</span>
      <br><span class="sub">${esc(r.matchText)} match -- ${esc(r.evidence)}</span></li>`).join("")}</ul>`;
}

/// One item: its axes with evidence, what was asked, and what can be done.
/// The item modal's security block (`#170`): the flag's state and reason,
/// then -- once a person has decided -- who and on what evidence. Empty for
/// an item nobody has ever flagged.
function securityBlock(card) {
  const flag = securityFlag(card);
  if (!flag) return "";
  const decided = flag.decided_by && flag.decided_at
    ? `<br><span class="sub">decided by ${esc(flag.decided_by)} at ${esc(flag.decided_at)}${flag.evidence ? `: ${esc(flag.evidence)}` : ""}</span>`
    : "";
  return `<p>${securityBadge(card)} ${flag.reason ? esc(flag.reason) : `<span class="sub">no reason given</span>`}
    <br><span class="sub">flagged by ${esc(flag.flagged_by)} at ${esc(flag.flagged_at)}</span>${decided}</p>
    ${clockDeadlinesBlock(card)}`;
}

/// The item modal's own reporting-clock block (`#157`/`#170` phase 2): each
/// deadline's due time, state and -- once one exists -- who submitted and
/// when. Read through the router (`GET /api/policy/clock`), never computed
/// here; empty for anything `clockDeadlinesHtml` would also skip.
function clockDeadlinesBlock(card) {
  if (!clock) return "";
  const item = reportDeadlines(clockIdx, card, nowFromClock(clock));
  if (!item) return "";
  return `<label>CRA Art. 14 reporting clock <span class="sub">${item.awaitingMeasure ? "final report: awaiting corrective-measure evidence" : `corrective measure available ${esc(item.correctiveMeasure.available_at)}; evidence: ${esc(item.correctiveMeasure.evidence)}`}</span></label>
    <ul class="sub">${item.deadlines
      .map(
        (d) => {
          const submission = d.submission ? `<br>submitted by ${esc(d.submission.by)} at ${esc(d.submission.at)}` : "";
          return `<li><span class="badge s-${esc(d.state)}">${esc(d.stateLabel)}</span> ${esc(d.label)}, due ${esc(d.dueAt)} -- ${esc(d.text)}
            ${submission}</li>`;
        },
      )
      .join("")}</ul>`;
}

/// The item modal's outbound block (`#171`): the GitHub publish state, the
/// comment link once posted, and any labels the repository does not have or
/// the last attempt's error. Empty for an item with nothing outbound yet.
function outboundBlock(card) {
  const info = outboundInfo(card);
  if (!info) return "";
  const label = OUTBOUND_STATE_LABELS[info.state] || info.state;
  return `<p><span class="tag">GitHub: ${esc(label)}</span>${info.commentUrl
      ? ` <a href="${esc(info.commentUrl)}" target="_blank" rel="noopener">comment</a>`
      : ""}
    ${info.labelsSkipped.length ? `<br><span class="sub">labels skipped (missing in the repository): ${esc(info.labelsSkipped.join(", "))}</span>` : ""}
    ${info.error ? `<br><span class="sub">${esc(info.error)}</span>` : ""}</p>`;
}

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
      <p class="sub">${esc(card.stage.replace("_", " "))} · ${esc(card.scope)} · from ${esc(card.requester)} (${esc(sourceText(card.source))}${card.source.reference ? `: ${esc(card.source.reference)}` : ""}) · waiting ${esc(fmtAge(card.age_seconds))}</p>
      ${securityBlock(card)}
      ${outboundBlock(card)}
      ${t ? `<p><span class="badge ik-p ik-${esc(t.priority)}">${esc(t.priority)}</span> <span class="tag">${esc(t.assessment.category)}</span>
        impact ${esc(t.assessment.impact)} × urgency ${esc(t.assessment.urgency)} · complexity ${esc(t.assessment.complexity)} · ${esc(estimateText(t.estimate))}
        · route ${esc(routeText(t.assessment.routing))}
        <br><span class="sub">assessed by ${esc(t.by)}: ${esc(t.verdict.verdict.replace("_", "-"))}${basisText(t.estimate_basis) ? ` · ${esc(basisText(t.estimate_basis))}` : ""}</span></p>
        ${t.assessment.summary ? `<p>${esc(t.assessment.summary)}</p>` : ""}` : `<p class="sub">Not assessed yet.</p>`}
      <table class="ik-axes"><tbody>${marks.map(m => `
        <tr class="ik-${m.mark}"><td>${MARK_TEXT[m.mark]}</td><th scope="row">${esc(m.label)}</th>
          <td>${m.evidence ? esc(m.evidence) : `<span class="sub">${esc(m.pass_condition)}</span>`}${m.cost ? ` <span class="sub">(cost ${esc(m.cost)})</span>` : ""}</td></tr>`).join("")}
      </tbody></table>
      ${card.questions && card.questions.length ? `<label>Asked of the requester</label><ul>${card.questions.map(q => `<li>${esc(q)}</li>`).join("")}</ul>` : ""}
      ${proposedSplit(t)}
      ${candidatesBlock(card)}
      ${nextActionsBlock(card)}
      ${card.decision ? `<p class="sub">decided ${esc(card.decision.decision.decision.replace("_", "-"))} by ${esc(card.decision.by)}</p>` : ""}
      <div class="row-btns" style="margin-top:16px">
        ${actions.map(a => `<button class="btn ${a === "release" || a === "publish" ? "primary" : a === "wontfix" ? "danger" : ""}" data-act="${a}">${esc(ACTION_LABELS[a])}</button>`).join("")}
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
  if (action === "flag_security") return openFlagSecurityDialog(card);
  if (action === "security_confirm") return openSecurityDecisionDialog("confirm", card);
  if (action === "security_dismiss") return openSecurityDecisionDialog("dismiss", card);
  if (action === "publish") return openPublishDialog(card);
  return openDecideDialog(action, card);
}

/// Approve and post a decided GitHub item's triage comment and labels
/// (`#171`). Factory never does this on its own -- this dialog's one button
/// is the approval. Shows the last attempt's state, if there was one, so a
/// retry after a `gh` failure is not a shot in the dark.
function openPublishDialog(card) {
  const info = outboundInfo(card);
  dialog("Approve and post to GitHub", card.title, `
    <p class="env-note">Posts the decided triage comment and applies the labels to the GitHub issue this item
      came from. Factory never does this on its own -- this is the approval.</p>
    ${info ? `<p class="sub">last attempt: ${esc(OUTBOUND_STATE_LABELS[info.state] || info.state)}${info.commentUrl
        ? ` -- <a href="${esc(info.commentUrl)}" target="_blank" rel="noopener">comment</a>`
        : ""}</p>
      ${info.labelsSkipped.length ? `<p class="sub">labels skipped (missing in the repository): ${esc(info.labelsSkipped.join(", "))}</p>` : ""}
      ${info.error ? `<p class="sub">${esc(info.error)}</p>` : ""}` : ""}`,
  "Approve and post",
  () => publishRequest(card.id));
}

function openFlagSecurityDialog(card) {
  dialog("Flag as a possible security report", card.title, `
    <p class="env-note">Moves it to the front of the queue in every open column. Nothing may release, split or
      close it until a person confirms or dismisses the flag.</p>
    <label for="ik-sec-reason">Reason <span class="sub">why this might be a security report</span></label>
    <textarea id="ik-sec-reason" rows="3"></textarea>`,
  "Flag it",
  () => flagSecurityRequest(card.id, $("ik-sec-reason").value));
}

/// The owner's confirm or dismiss (`#170`) -- the daemon refuses anyone else,
/// whatever role they hold; the UI itself sends no token, so this is always
/// the owner asking. Evidence is required to dismiss, optional to confirm.
function openSecurityDecisionDialog(verdict, card) {
  const title = verdict === "confirm" ? "Confirm security report" : "Dismiss security report";
  const evidenceHint = verdict === "confirm" ? "optional" : "what clears it -- required";
  dialog(title, card.title, `
    <p class="env-note">${verdict === "confirm"
      ? "Marks it a real security report. It journals the decision and stays evidence even if the task is later released or closed."
      : "Marks it not a security report after all. Releasing, splitting and closing it work normally again."}</p>
    <label for="ik-sec-evidence">Evidence <span class="sub">${evidenceHint}</span></label>
    <textarea id="ik-sec-evidence" rows="3"></textarea>`,
  title,
  () => securityDecisionRequest(card.id, verdict, $("ik-sec-evidence").value),
  () => securityDecisionProblem(verdict, $("ik-sec-evidence") ? $("ik-sec-evidence").value : ""),
  verdict === "dismiss");
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
    <label for="ik-ref">Reference <span class="sub">optional: an issue URL, a mail id</span></label><input id="ik-ref">
    <label class="checkrow"><input type="checkbox" id="ik-security"><span>Possible security report -- moves it to the front of the queue and holds it until a person confirms or dismisses it</span></label>`,
  "Hand in",
  () => addRequest({ title: $("ik-title").value, instructions: $("ik-text").value, scope: $("ik-scope").value,
    requester: $("ik-requester").value, reference: $("ik-ref").value, security: $("ik-security").checked }),
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
      as ${esc(t.assessment.category)}, ${esc(t.priority)}, ${esc(estimateText(t.estimate))}${basisText(t.estimate_basis) ? ` (${esc(basisText(t.estimate_basis))})` : ""}. The triage verdict is journaled as its first attestation.</p>
      ${t.assessment.routing.workflow ? "" : `<label class="checkrow"><input type="checkbox" id="ik-run"><span>Run it now</span></label>`}`;
  } else if (action === "needs_info") {
    const suggested = (card.triage && card.triage.assessment.questions) || [];
    body = `<p class="env-note">Sends it back to ${esc(card.requester)}. It stays in intake until information comes.</p>
      <label for="ik-questions">Questions <span class="sub">one per line; empty asks the assessment's, or its failed axes</span></label>
      <textarea id="ik-questions" rows="4">${esc(suggested.join("\n"))}</textarea>`;
  } else {
    // Prefilled from the assessment's own confirmed duplicate, when it has
    // one -- what it named is what closes the item, not a fresh guess.
    const draft = wontfixDraft(card);
    body = `<p class="env-note">Closes it. Only for a verified duplicate, an invalid report or something that is not ours to do.</p>
      <label for="ik-reason">Reason</label><select id="ik-reason"><option value="">…</option>${WONTFIX_REASONS.map(r => `<option value="${r.key}"${r.key === draft.reason ? " selected" : ""}>${esc(r.label)}</option>`).join("")}</select>
      <label for="ik-evidence">Evidence <span class="sub">what verifies it</span></label><textarea id="ik-evidence" rows="3">${esc(draft.evidence)}</textarea>
      <label for="ik-dup">Duplicate of <span class="sub">for a duplicate: a task id or an issue URL</span></label><input id="ik-dup" value="${esc(draft.duplicate_of)}">`;
  }
  dialog(ACTION_LABELS[action], card.title, body, ACTION_LABELS[action],
    () => decideRequest(card.id, action, values()),
    () => decideProblem(action, values()),
    action === "wontfix");
}

/// `duplicates` verdict rows for the assess dialog: the daemon's own list
/// (`card.candidates`), each with its current verdict and evidence -- the
/// last answer given, or the daemon's own found-reason to start from.
function dupeVerdictOptions(selected) {
  return ["unverified", "confirmed", "rejected"]
    .map(v => `<option value="${v}"${v === selected ? " selected" : ""}>${v}</option>`)
    .join("");
}

function dupesHtml(candidates) {
  if (!candidates.length) return "";
  return `<label>Possible duplicates <span class="sub">confirm or reject each, with your own evidence</span></label>
    <div id="ik-dupes">${candidates.map((c, i) => `
      <div class="ik-dupe-row" data-dupe="${i}">
        <p><span class="tag">${esc(c.kind)}</span> <code>${esc(c.reference)}</code> ${esc(c.title)}
          <span class="sub">(${esc(c.match)}${c.match === "text" && c.score != null ? ` ${esc(c.score)}%` : ""})</span></p>
        <div class="ik-grid">
          <label>Verdict <select data-dupe-verdict>${dupeVerdictOptions(c.verdict)}</select></label>
          <label>Evidence <input data-dupe-evidence value="${esc(c.evidence)}"></label>
        </div>
      </div>`).join("")}</div>`;
}

/// A single row to name a duplicate the search never found -- a vault page
/// from the triager's own `factory knowledge search`. Left blank, it is
/// dropped (`buildDuplicateAnswer`/`buildAssessment`).
function knowledgeRowHtml() {
  return `<label>Add a knowledge candidate <span class="sub">a vault page \`factory knowledge search\` found; leave blank if none</span></label>
    <div class="ik-grid">
      <label>Vault path <input id="ik-know-ref" placeholder="specs/some-decision.md"></label>
      <label>Title <input id="ik-know-title"></label>
    </div>
    <label>Evidence <input id="ik-know-evidence"></label>`;
}

function readDupeRows(candidates) {
  return candidates.map((c, i) => ({
    kind: c.kind,
    reference: c.reference,
    title: c.title,
    match: c.match,
    score: c.score,
    verdict: document.querySelector(`.scrim [data-dupe="${i}"] [data-dupe-verdict]`).value,
    evidence: document.querySelector(`.scrim [data-dupe="${i}"] [data-dupe-evidence]`).value,
  }));
}

function readKnowledgeRow() {
  const ref = $("ik-know-ref") ? $("ik-know-ref").value.trim() : "";
  if (!ref) return null;
  return {
    kind: "knowledge",
    reference: ref,
    title: $("ik-know-title") ? $("ik-know-title").value : "",
    match: "text",
    // No score of its own: it was found by reading, not scored by overlap.
    // Blank rather than 0 -- `buildDuplicateAnswer` then leaves it out
    // rather than wire a false "0% match".
    score: "",
    verdict: "confirmed",
    evidence: $("ik-know-evidence") ? $("ik-know-evidence").value : "",
  };
}

/// The routed scope's own extra checks (`#169`, `route.definition.checks`),
/// as a row per check -- `openAssessDialog`'s axes rows, without the cost
/// column, and with a note when a check applies to only some categories.
/// Empty for a scope whose chain adds none, the common case.
function checksHtml(route, prior) {
  const checks = (route.definition && route.definition.checks) || [];
  if (!checks.length) return "";
  const priorCheck = (id) => (prior && prior.checks || []).find(c => c.id === id) || { pass: true, evidence: "" };
  const rows = checks.map(c => {
    const p = priorCheck(c.id);
    const note = c.categories && c.categories.length ? ` <span class="sub">(${esc(c.categories.join(", "))} only)</span>` : "";
    return `<tr><th scope="row"><label class="checkrow"><input type="checkbox" data-check="${esc(c.id)}"${p.pass ? " checked" : ""}><span>${esc(c.id)}${note}</span></label></th>
      <td><input data-check-evidence="${esc(c.id)}" data-check-categories="${esc((c.categories || []).join(","))}"
        placeholder="${esc(c.pass_condition)}" value="${esc(p.evidence)}"></td></tr>`;
  }).join("");
  return `<label>${esc(route.scope)}'s own definition of ready adds these checks -- leave one out of your
      category and it is dropped, never submitted</label>
    <table class="ik-axes ik-form"><tbody>${rows}</tbody></table>`;
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
  const candidates = card.candidates || [];
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
    duplicates: (() => {
      const knowledge = readKnowledgeRow();
      return knowledge ? [...readDupeRows(candidates), knowledge] : readDupeRows(candidates);
    })(),
    // The routed scope's own extra checks (`#169`) -- `buildAssessment`
    // drops one that does not apply to the category chosen above.
    checks: routeFor(board, $("ik-route").value).definition?.checks?.map(c => ({
      id: c.id,
      categories: c.categories || [],
      pass: document.querySelector(`.scrim [data-check="${c.id}"]`)?.checked ?? true,
      evidence: document.querySelector(`.scrim [data-check-evidence="${c.id}"]`)?.value || "",
    })) || [],
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
    <div id="ik-checks"></div>
    <label for="ik-summary">Summary</label><textarea id="ik-summary" rows="2">${esc(prior ? prior.summary : "")}</textarea>
    <label for="ik-ask">Questions for a needs-info <span class="sub">one per line</span></label>
    <textarea id="ik-ask" rows="2">${esc(prior ? (prior.questions || []).join("\n") : "")}</textarea>
    ${dupesHtml(candidates)}
    ${knowledgeRowHtml()}
    <p class="ik-preview" id="ik-preview" aria-live="polite"></p>
    <label class="checkrow"><input type="checkbox" id="ik-decide" checked><span>Apply the verdict: release if ready, send back if needs-info</span></label>`,
  "Record assessment",
  () => assessRequest(card.id, buildAssessment(values()), $("ik-decide").checked),
  () => {
    const a = buildAssessment(values());
    const definition = routeFor(board, a.routing.scope).definition;
    const v = previewVerdict(a, definition);
    const pv = $("ik-preview");
    if (pv) {
      pv.textContent = `${v.verdict === "ready" ? "ready" : `needs-info (${v.blockers.join(", ")})`} · ${priorityOf(a.impact, a.urgency) || "-"} · ${estimateText(estimateOf(a.complexity))}`;
    }
    return assessmentProblem(a, candidates, definition) || routeProblem(a, board);
  });
  // How it runs there: an agent, or a workflow with its inputs and an
  // agent -- and so a model -- per step, and its own extra checks (`#169`).
  // Redrawn when the scope or the workflow changes, keeping what was chosen
  // where it still applies.
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
    $("ik-checks").innerHTML = checksHtml(r, prior);
    for (const el of document.querySelectorAll(".scrim #ik-route-how input, .scrim #ik-route-how select, .scrim #ik-checks input")) {
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
