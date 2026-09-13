//! Process-level workflow list, canvas and task-node inspector.
//!
//! Every decision that does not need the DOM lives in `workflow-model.js`
//! and `workflow-graph.js`; this file only ever paints what those decide and
//! wires the interactions that decide it. Two render paths matter:
//! `renderEditor`/`renderCanvas` rebuild structure (a different workflow, a
//! node added or removed, a mode switch) and `paintStatuses` only updates
//! status-dependent attributes on elements that already exist -- the
//! `workflow_run_updated` socket event, which can arrive several times a
//! second while a run is live, only ever calls the latter, so it never
//! steals focus from a canvas node or an inspector field mid-edit.

import { $, api, esc, state } from "./core.js";
import { inScope, writeHash } from "./scopes.js";
import { openTask } from "./tasks.js";
import {
  connectionError,
  edgeStatusClass,
  fitView,
  freePosition,
  isDrag,
  nodeStatusClass,
  rootIds,
  topologicalSummary,
} from "./workflow-graph.js";
import {
  addEdge,
  applyWorkflowEvent,
  blankWorkflow,
  duplicateNode,
  editorButtons,
  newTaskNode,
  openTaskAction,
  readWorkflowRouteTail,
  validate,
  workflowRouteTail,
} from "./workflow-model.js";

const uid = prefix =>
  `${prefix}-${globalThis.crypto?.randomUUID?.() || Math.random().toString(36).slice(2)}`;
const number = id => { const value = $(id).value.trim(); return value ? Number(value) : null; };

// -------------------------------------------------------------------- state
//
// `current` is the live, editable definition -- Design mode's only source of
// truth. `currentRun` is whichever run is selected, which Run mode draws
// from `currentRun.definition` (its immutable snapshot) instead: U4 is
// explicit that Design mode must never map a run's statuses onto nodes that
// may have changed since, so the two views never share a graph object.

let workflows = [];
let runs = [];
let current = null;
let currentRun = null;
let dirty = false;
let notice = null;
let mode = "design"; // "design" | "run"
let selectedNode = null;
let selectedEdge = null;
let connectFrom = null;
let view = { x: 40, y: 40, zoom: 1 };
let saving = false;
let clientErrors = [];
let serverError = null;
let pendingDiscard = null; // a () => void to run if a dirty-switch is confirmed
let inspectorRenderedFor = null; // the selection key `renderInspector` last wrote values for
let wired = false;

function activeGraph() {
  return mode === "run" && currentRun ? currentRun.definition : current;
}

function overlayFor(nodeId) {
  return mode === "run" && currentRun ? currentRun.nodes.find(node => node.node_id === nodeId) : null;
}

function markDirty() {
  if (dirty) return;
  dirty = true;
  notice = null;
  renderButtons();
  $("workflow-dirty").hidden = false;
}

function clearDirty() {
  dirty = false;
  $("workflow-dirty").hidden = true;
}

/// R3/R4: everything that belongs to "a workflow is open" and nothing that
/// belongs to the app as a whole. Used wherever the open workflow stops
/// existing out from under the view -- scope filtering it out, deleting it,
/// or a `workflow_deleted` event naming it -- rather than a deliberate
/// navigation to something else that would set its own new state right
/// after. "Recent runs" and the selection are the open workflow's alone;
/// leaving either behind once nothing is open reads as if something still
/// were.
function closeWorkflow() {
  current = null; currentRun = null;
  selectedNode = null; selectedEdge = null; connectFrom = null;
  runs = []; inspectorRenderedFor = null;
  clearDirty();
}

// ---------------------------------------------------------------- routing

export function workflowTail() {
  return current?.id ? workflowRouteTail(current.id, mode === "run" ? currentRun?.id : null) : [];
}

/// The router calls this without awaiting it (its `read` contract is
/// fire-and-forget everywhere else in the app too), but the tail it reads
/// only becomes true once `open`'s own `loadRuns` resolves. `showTab` writes
/// the hash again right after calling this, while `current` is still
/// whatever it was before -- for a fresh link that's `null`, so that write
/// would otherwise replace the very tail this function was just asked to
/// apply with an empty one. Settling here and correcting the hash
/// afterward (a correction, not a new navigation: `replace`) is what
/// `tasks.js`'s `openTask` already does for the same reason.
export async function readWorkflowTail(tail) {
  const { id, runId } = readWorkflowRouteTail(tail);
  if (!id) return;
  const found = workflows.find(workflow => workflow.id === id);
  if (found) await open(found, false, runId);
  else await loadWorkflows(id, runId);
  writeHash(true);
}

export async function loadWorkflows(wanted, wantedRun) {
  setBusy(true);
  try {
    workflows = (await api("/api/workflows")).workflows;
    if (wanted) {
      const found = workflows.find(workflow => workflow.id === wanted);
      if (found) await open(found, false, wantedRun);
    } else if (current?.id && !dirty) {
      const fresh = workflows.find(workflow => workflow.id === current.id);
      if (fresh) current = structuredClone(fresh);
    }
    renderWorkflows();
  } catch (error) { showServerError(error); }
  finally { setBusy(false); }
}

export function renderWorkflows() {
  if (current && !inScope(current.scope)) {
    closeWorkflow();
    // R4: `scopes.js`'s rail `select()` writes the hash *before* calling
    // back in here, so it wrote the outgoing scope's tail -- correct it now
    // that the workflow it named is actually closed. `replaceState` fires
    // no `hashchange`, so this cannot loop back into another rerender.
    writeHash(true);
  }
  const visible = workflows.filter(workflow => inScope(workflow.scope));
  $("workflow-list").innerHTML = visible.length ? visible.map(workflow => `
    <button class="workflow-list-item${current?.id === workflow.id ? " on" : ""}" data-id="${esc(workflow.id)}">
      <strong>${esc(workflow.name)}</strong><span>${esc(workflow.scope)} · r${workflow.revision}</span>
    </button>`).join("") : `<div class="empty">No workflows in this scope.</div>`;
  for (const button of $("workflow-list").querySelectorAll("[data-id]")) {
    button.onclick = () => guardDirty(() => open(workflows.find(workflow => workflow.id === button.dataset.id)));
  }
  $("workflow-new").disabled = state.scope === null;
  $("workflow-new").title = state.scope === null ? "Select one owning scope first" : "";
  renderEditor();
}

// ------------------------------------------------------- unsaved-change guard

/// U3: switching workflows or starting a new one while dirty must not
/// silently discard the edit. `proceed` runs immediately when there is
/// nothing to lose; otherwise an inline choice appears where errors do
/// (never a native `confirm()` -- see the delete button for the one
/// exception this app already made peace with).
function guardDirty(proceed) {
  if (!dirty) { proceed(); return; }
  pendingDiscard = proceed;
  renderProblems();
}

async function open(workflow, navigate = true, runId) {
  current = structuredClone(workflow);
  currentRun = null;
  selectedNode = null; selectedEdge = null; connectFrom = null;
  clearDirty(); notice = null; clientErrors = []; serverError = null; pendingDiscard = null;
  mode = "design";
  const canvas = $("workflow-canvas");
  view = fitView(current.nodes, canvas?.clientWidth || 800, canvas?.clientHeight || 500);
  renderWorkflows();
  await loadRuns(runId);
  if (navigate) writeHash();
}

function newWorkflow() {
  if (!state.scope) return;
  guardDirty(() => {
    current = blankWorkflow(state.scope);
    current.nodes.push(newTaskNode(state.scope));
    selectedNode = current.nodes[0].id;
    currentRun = null; mode = "design"; notice = null; clientErrors = []; serverError = null;
    runs = []; inspectorRenderedFor = null; // R3: a brand new workflow has no runs to inherit
    clearDirty();
    renderWorkflows();
    writeHash();
  });
}

// -------------------------------------------------------------- structure

function renderEditor() {
  const has = !!current;
  $("workflow-node-fields").hidden = !has || !selectedNode;
  $("workflow-run-panel").hidden = !has || mode !== "run";
  if (!has) {
    for (const id of ["workflow-name", "workflow-description", "workflow-scope"]) $(id).value = "";
    $("workflow-nodes").innerHTML = ""; $("workflow-edges").innerHTML = "";
    $("workflow-summary").innerHTML = ""; $("workflow-edge-summary").innerHTML = "";
    $("workflow-revision").textContent = ""; $("workflow-run-status").innerHTML = "";
    inspectorRenderedFor = null;
    renderButtons(); renderProblems(); renderRunsList();
    return;
  }
  $("workflow-revision").textContent = current.revision ? `definition revision ${current.revision}` : "not saved yet";
  for (const button of $("workflow-mode").querySelectorAll("[data-mode]")) {
    button.classList.toggle("on", button.dataset.mode === mode);
    button.disabled = button.dataset.mode === "run" && !currentRun;
  }
  $("workflow-canvas").classList.toggle("wf-run-mode", mode === "run");
  renderCanvas();
  renderInspector();
  renderSummary();
  renderRunPanel();
  renderButtons();
  renderProblems();
  renderRunsList();
}

function runNode(id) { return overlayFor(id); }

function edgeHint() {
  if (mode === "run") return "Run mode shows what actually happened; it is read-only.";
  if (connectFrom) return `Click another card, or press Enter on one, to link from "${nodeTitleOf(connectFrom)}". Esc cancels.`;
  if (selectedEdge) return `A link is selected. Delete removes it, or use "Delete link".`;
  return `Click a card's "+" to link it to another. Click a link to select it, then Delete removes it.`;
}

function nodeTitleOf(id) {
  return activeGraph()?.nodes.find(node => node.id === id)?.task.title || id;
}

function renderCanvas() {
  const graph = activeGraph();
  const focused = document.activeElement?.dataset?.node;
  $("workflow-world").style.transform = `translate(${view.x}px,${view.y}px) scale(${view.zoom})`;
  $("workflow-zoom-reset").textContent = `${Math.round(view.zoom * 100)}%`;
  const roots = new Set(rootIds(graph.nodes, graph.edges));

  $("workflow-edges").setAttribute("viewBox", "0 0 2400 1600");
  $("workflow-edges").innerHTML = `<defs>
      <marker id="workflow-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" /></marker>
      <marker id="workflow-arrow-done" class="wf-edge-done" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" /></marker>
    </defs>` +
    graph.edges.map(edge => {
      const from = graph.nodes.find(node => node.id === edge.from), to = graph.nodes.find(node => node.id === edge.to);
      if (!from || !to) return "";
      const x1 = from.position.x + 184, y1 = from.position.y + 42, x2 = to.position.x, y2 = to.position.y + 42;
      const bend = Math.max(45, Math.abs(x2 - x1) * .45);
      const d = `M${x1},${y1} C${x1 + bend},${y1} ${x2 - bend},${y2} ${x2},${y2}`;
      return `<g data-edge="${esc(edge.id)}">
          <path class="workflow-edge-hit" d="${d}"/>
          <path class="workflow-edge" data-id="${esc(edge.id)}" d="${d}" marker-end="url(#workflow-arrow)"/>
        </g>`;
    }).join("");

  $("workflow-nodes").innerHTML = graph.nodes.map(node => `
    <div class="workflow-node" data-node="${esc(node.id)}" tabindex="0" role="group"
      aria-label="${esc(node.task.title || "Untitled task")}${roots.has(node.id) ? ", start node" : ""}"
      style="left:${node.position.x}px;top:${node.position.y}px">
      ${roots.has(node.id) ? `<span class="wf-start">START</span>` : ""}
      <span class="wf-port-in" aria-hidden="true"></span>
      <span class="workflow-node-kind">TASK</span>
      <strong>${esc(node.task.title || "Untitled task")}</strong>
      <span class="wf-line">${esc(node.task.scope || "")} · ${esc(node.task.agent || "default agent")}</span>
      <span class="wf-badge-row"><span class="wf-badge" data-role="badge"></span></span>
      <button type="button" class="wf-open-task" data-role="open-task" hidden>Open task ↗</button>
      <button type="button" class="wf-port-out" data-port="${esc(node.id)}" aria-label="Link from ${esc(node.task.title || "this node")}">+</button>
    </div>`).join("");
  for (const node of $("workflow-nodes").querySelectorAll("[data-node]")) wireNode(node);
  for (const group of $("workflow-edges").querySelectorAll("[data-edge]")) wireEdge(group);

  paintStatuses();
  $("workflow-hint").textContent = edgeHint();

  if (focused) $("workflow-nodes").querySelector(`[data-node="${CSS.escape(focused)}"]`)?.focus();
}

/// Status-dependent DOM only: class names, badge text, edge colour, the
/// open-task button's visibility, the run-status chip and the run panel.
/// Called after every structural render and on every `workflow_run_updated`
/// -- it never rebuilds an element, so it never steals focus or a caret.
function paintStatuses() {
  if (!current) return;
  const graph = activeGraph();
  for (const node of graph.nodes) {
    const el = $("workflow-nodes")?.querySelector(`[data-node="${CSS.escape(node.id)}"]`);
    if (!el) continue;
    const execution = runNode(node.id);
    const status = execution?.status || "unstarted";
    el.className = `workflow-node ${nodeStatusClass(status)}`;
    if (selectedNode === node.id) el.classList.add("selected");
    if (connectFrom === node.id) el.classList.add("wf-connecting");
    const badge = el.querySelector('[data-role="badge"]');
    if (badge) { badge.className = `wf-badge ${nodeStatusClass(status)}`; badge.textContent = status; }
    const openBtn = el.querySelector('[data-role="open-task"]');
    if (openBtn) {
      const action = openTaskAction(execution, openTask);
      openBtn.hidden = !action;
      if (action) { openBtn.dataset.task = execution.task_id; openBtn.onclick = event => { event.stopPropagation(); action(); }; }
    }
  }
  for (const edge of graph.edges) {
    const from = graph.nodes.find(node => node.id === edge.from);
    const status = from ? (runNode(from.id)?.status || "unstarted") : "unstarted";
    const path = $("workflow-edges")?.querySelector(`path.workflow-edge[data-id="${CSS.escape(edge.id)}"]`);
    if (!path) continue;
    const cls = ["workflow-edge", edgeStatusClass(status)];
    if (selectedEdge === edge.id) cls.push("wf-selected");
    path.setAttribute("class", cls.filter(Boolean).join(" "));
    path.setAttribute("marker-end", status === "done" ? "url(#workflow-arrow-done)" : "url(#workflow-arrow)");
  }
  paintSummaryStatuses();
  renderRunStatusChip();
}

function wireNode(element) {
  // No card-level `onclick`: R1 found that Chrome dispatches a zero-distance
  // `pointermove` right after `setPointerCapture` on an ordinary click, so a
  // naive "moved -> drag" flag misreads every click as a drag and
  // `onpointerup` re-renders the canvas before the browser gets to dispatch
  // `click` -- which then has nowhere to land. `onpointerup` below is the
  // single path for both a click (select/link) and a drag (move); there is
  // no fallback relying on `click` finding this element afterward.
  element.onkeydown = event => {
    // Enter/Space select (or, mid-connect, complete a link) in either mode
    // -- Run mode is read-only about the *definition*, not about which node
    // is selected to inspect. Everything below this is a mutation, so it
    // stays behind the mode check.
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      activateNode(element.dataset.node);
      return;
    }
    if (mode === "run") return;
    if (event.key === "Delete" || event.key === "Backspace") {
      event.preventDefault();
      deleteNode(element.dataset.node); // R2: the focused card, not `selectedNode`
      return;
    }
    if (event.key === "Escape") { event.preventDefault(); cancelConnect(); return; }
    const delta = event.shiftKey ? 20 : 5;
    const move = { ArrowLeft: [-delta, 0], ArrowRight: [delta, 0], ArrowUp: [0, -delta], ArrowDown: [0, delta] }[event.key];
    if (!move) return;
    event.preventDefault();
    const node = current.nodes.find(item => item.id === element.dataset.node);
    node.position.x += move[0]; node.position.y += move[1];
    markDirty();
    renderCanvas();
    $("workflow-nodes").querySelector(`[data-node="${CSS.escape(node.id)}"]`)?.focus();
  };
  // R2/R5: landing keyboard focus on a card selects it in either mode --
  // `focusin` bubbles from the port/open-task buttons inside the card too,
  // `focus` would not.
  // `onfocusin` is not a real IDL event handler property (unlike `onfocus`,
  // `focusin`/`focusout` were never added to `GlobalEventHandlers`) --
  // assigning to it silently does nothing, so this has to be
  // `addEventListener`. Confirmed the hard way: `element.onfocusin = ...`
  // never fired even though focus genuinely moved.
  element.addEventListener("focusin", () => focusNode(element.dataset.node));
  const port = element.querySelector("[data-port]");
  if (port) {
    port.onclick = event => {
      event.stopPropagation();
      if (mode === "run") return;
      connectFrom = element.dataset.node;
      selectedEdge = null;
      renderCanvas();
    };
  }
  element.onpointerdown = event => {
    // Checked, and must stay, before `setPointerCapture`: capturing the
    // pointer on the card first would swallow the port/open-task button's
    // own click. The port itself is hidden in Run mode (see app.css), so
    // this exclusion only ever matters in Design mode.
    if (event.button !== 0 || event.target.closest('[data-role="open-task"], [data-port]')) return;
    // Run mode never repositions a node -- there is nothing to drag -- but
    // a click there still selects it (R5), so pointer handling stays wired
    // rather than bailing out the way keyboard movement does.
    const node = mode === "run" ? null : current.nodes.find(item => item.id === element.dataset.node);
    const start = { x: event.clientX, y: event.clientY };
    const origin = node ? { x: node.position.x, y: node.position.y } : null;
    let dragging = false;
    element.setPointerCapture(event.pointerId);
    element.onpointermove = move => {
      if (!node) return;
      const at = { x: move.clientX, y: move.clientY };
      if (!dragging && !isDrag(start, at)) return; // R1: a real drag, not the incidental pointermove
      dragging = true;
      node.position.x = Math.round(origin.x + (at.x - start.x) / view.zoom);
      node.position.y = Math.round(origin.y + (at.y - start.y) / view.zoom);
      element.style.left = `${node.position.x}px`;
      element.style.top = `${node.position.y}px`;
    };
    element.onpointerup = () => {
      element.onpointermove = null;
      if (dragging) { markDirty(); renderCanvas(); return; }
      activateNode(element.dataset.node);
    };
  };
}

function wireEdge(group) {
  const id = group.dataset.edge;
  group.querySelector(".workflow-edge-hit").onclick = event => {
    event.stopPropagation();
    if (mode === "run") return;
    selectedEdge = selectedEdge === id ? null : id;
    connectFrom = null;
    paintStatuses();
    $("workflow-hint").textContent = edgeHint();
  };
}

/// Landing on `id` selects it, without rebuilding the canvas (R2): a full
/// `renderEditor()` here would replace the very element that just received
/// focus. `paintStatuses` (class names only) plus `renderInspector` is
/// enough, since nothing else about the structure changed. The
/// `selectedNode === id` guard is load-bearing, not an optimisation: without
/// it, tabbing across a card that is already selected would re-render the
/// inspector on every focus event and fight whatever is being typed there.
function focusNode(id) {
  if (selectedNode === id) return;
  selectedNode = id; selectedEdge = null;
  paintStatuses();
  renderInspector();
  renderButtons();
}

/// A click or an explicit Enter/Space -- as opposed to focus merely landing
/// somewhere while tabbing through. While connecting, this is what
/// completes (or refuses) the link; `completeConnect` alone decides whether
/// that moves the selection, since a refused link must leave it exactly as
/// it was (see `completeConnect`'s own comment).
function activateNode(id) {
  if (connectFrom) { completeConnect(id); return; }
  focusNode(id);
}

/// Selecting from the accessible summary (U8) rather than the card itself:
/// nothing there already has DOM focus on the card, so it is moved there
/// explicitly once the selection (or a completed connection) has settled.
function selectFromSummary(id) {
  activateNode(id);
  $("workflow-nodes").querySelector(`[data-node="${CSS.escape(id)}"]`)?.focus();
}

function completeConnect(toId) {
  const fromId = connectFrom;
  connectFrom = null;
  if (!fromId) return;
  const error = connectionError(current.nodes, current.edges, fromId, toId);
  if (error) { clientErrors = [{ message: error }]; renderCanvas(); renderProblems(); return; }
  current.edges = addEdge(current.edges, uid("edge"), fromId, toId);
  clientErrors = [];
  markDirty();
  selectedNode = toId;
  renderEditor();
}

function cancelConnect() {
  if (!connectFrom) return;
  connectFrom = null;
  renderCanvas();
}

// -------------------------------------------------------------- inspector

function renderInspector() {
  const readOnly = mode === "run";
  // R5: Run mode reads the run's own immutable snapshot -- `activeGraph()`,
  // the same source `renderCanvas` draws from -- never the live `current`,
  // which may have been renamed/re-shaped by edits or later revisions since
  // this run started.
  const graph = activeGraph();
  const node = graph.nodes.find(item => item.id === selectedNode);
  $("workflow-node-fields").hidden = !node;
  const key = `${mode}:${graph.id}:${selectedNode || ""}:${graph.revision}:${currentRun?.id || ""}`;
  const selectionChanged = key !== inspectorRenderedFor;
  for (const id of ["workflow-name", "workflow-description", "workflow-node-title", "workflow-node-instructions", "workflow-node-agent", "workflow-node-worktree", "workflow-node-estimate", "workflow-node-ack", "workflow-node-timeout", "workflow-node-blocked"]) {
    $(id).disabled = readOnly;
  }
  $("workflow-scope").value = graph.scope;
  if (!selectionChanged) return;
  inspectorRenderedFor = key;
  $("workflow-name").value = graph.name;
  $("workflow-description").value = graph.description || "";
  if (!node) return;
  const agents = state.scopes.find(scope => scope.name === graph.scope)?.agents || [];
  const options = agents.map(agent => agent.name);
  // U10: an agent set through the API (an adapter name like `shell`) that is
  // not among the scope's declared agents must stay selectable, not vanish
  // into null the moment any other field is edited.
  if (node.task.agent && !options.includes(node.task.agent)) options.push(node.task.agent);
  $("workflow-node-agent").innerHTML = `<option value="">Default agent</option>` +
    options.map(name => `<option value="${esc(name)}">${esc(name)}</option>`).join("");
  $("workflow-node-scope").value = node.task.scope || graph.scope;
  $("workflow-node-title").value = node.task.title || "";
  $("workflow-node-instructions").value = node.task.instructions || "";
  $("workflow-node-agent").value = node.task.agent || "";
  $("workflow-node-worktree").checked = node.task.worktree !== false;
  $("workflow-node-estimate").value = node.task.estimate_seconds ?? "";
  $("workflow-node-ack").value = node.task.ack_timeout_seconds ?? "";
  $("workflow-node-timeout").value = node.task.timeout_seconds ?? "";
  $("workflow-node-blocked").value = node.task.blocked_timeout_seconds ?? "";
}

/// The model only, never `.trim()` -- trimming happens once, on save
/// (`save()`), so a trailing space typed a moment ago does not vanish out
/// from under the cursor while it is still being typed (U2).
function readEditor() {
  current.name = $("workflow-name").value;
  current.description = $("workflow-description").value;
  const node = current.nodes.find(item => item.id === selectedNode);
  if (node) {
    node.task.title = $("workflow-node-title").value;
    node.task.instructions = $("workflow-node-instructions").value;
    node.task.scope = current.scope;
    node.task.agent = $("workflow-node-agent").value || null;
    node.task.worktree = $("workflow-node-worktree").checked;
    node.task.estimate_seconds = number("workflow-node-estimate");
    node.task.ack_timeout_seconds = number("workflow-node-ack");
    node.task.timeout_seconds = number("workflow-node-timeout");
    node.task.blocked_timeout_seconds = number("workflow-node-blocked");
  }
}

function renderRunPanel() {
  if (mode !== "run" || !currentRun) { $("workflow-run-meta").innerHTML = ""; return; }
  const failed = currentRun.nodes.find(node => node.node_id === currentRun.failure_node_id);
  const failedTitle = failed && currentRun.definition.nodes.find(node => node.id === failed.node_id)?.task.title;
  $("workflow-run-meta").innerHTML = `<dl>
      <dt>Run</dt><dd>${esc(currentRun.id)}</dd>
      <dt>Status</dt><dd>${esc(currentRun.status)}</dd>
      <dt>Revision</dt><dd>${esc(currentRun.revision)}</dd>
      ${currentRun.failure_node_id ? `<dt>Failed node</dt><dd>${esc(failedTitle || currentRun.failure_node_id)}</dd>` : ""}
      ${currentRun.error ? `<dt>Error</dt><dd>${esc(currentRun.error)}</dd>` : ""}
    </dl>`;
}

// ------------------------------------------------------------------ summary

function renderSummary() {
  const graph = activeGraph();
  const summary = topologicalSummary(graph);
  if (summary.error) {
    $("workflow-summary").innerHTML = `<li class="err">${esc(summary.error)}</li>`;
  } else {
    $("workflow-summary").innerHTML = summary.nodes.map(item => {
      const node = graph.nodes.find(n => n.id === item.id);
      const after = item.after.length ? item.after.map(id => esc(graph.nodes.find(n => n.id === id)?.task.title || id)).join(", ") : "start";
      return `<li data-summary-node="${esc(item.id)}">
          <button type="button" data-select-node="${esc(item.id)}">${esc(node.task.title || item.id)}</button>
          <span class="wf-badge" data-role="badge"></span>
          <span> after ${esc(after)}</span>
          <button type="button" class="wf-open-task" data-role="open-task" hidden>Open task ↗</button>
        </li>`;
    }).join("");
  }
  $("workflow-edge-summary").innerHTML = graph.edges.map(edge => {
    const from = esc(graph.nodes.find(n => n.id === edge.from)?.task.title || edge.from);
    const to = esc(graph.nodes.find(n => n.id === edge.to)?.task.title || edge.to);
    return `<li data-summary-edge="${esc(edge.id)}">${from} → ${to}
        <button type="button" class="wf-edge-remove" data-remove-edge="${esc(edge.id)}"${mode === "run" ? " hidden" : ""}>Delete link</button>
      </li>`;
  }).join("");
  for (const button of $("workflow-summary").querySelectorAll("[data-select-node]")) {
    button.onclick = () => selectFromSummary(button.dataset.selectNode);
  }
  for (const button of $("workflow-edge-summary").querySelectorAll("[data-remove-edge]")) {
    button.onclick = () => deleteEdge(button.dataset.removeEdge);
  }
  paintSummaryStatuses();
}

function paintSummaryStatuses() {
  for (const li of $("workflow-summary")?.querySelectorAll("[data-summary-node]") || []) {
    const id = li.dataset.summaryNode;
    const execution = runNode(id);
    const status = execution?.status || "unstarted";
    const badge = li.querySelector('[data-role="badge"]');
    if (badge) { badge.className = `wf-badge ${nodeStatusClass(status)}`; badge.textContent = status; }
    const openBtn = li.querySelector('[data-role="open-task"]');
    if (openBtn) {
      const action = openTaskAction(execution, openTask);
      openBtn.hidden = !action;
      if (action) openBtn.onclick = action;
    }
  }
}

// --------------------------------------------------------- buttons/errors

function renderButtons() {
  const buttons = editorButtons({
    hasWorkflow: !!current, hasId: !!current?.id, dirty, saving, run: currentRun, mode,
  });
  $("workflow-save").disabled = buttons.save.disabled;
  $("workflow-save").classList.toggle("loading", buttons.save.loading);
  $("workflow-run").disabled = buttons.run.disabled;
  $("workflow-run").title = buttons.run.hint;
  $("workflow-cancel").disabled = buttons.cancel.disabled;
  const has = !!current;
  $("workflow-delete").disabled = !current?.id;
  $("workflow-connect").disabled = !has || !selectedNode || mode === "run";
  $("workflow-duplicate").disabled = !has || !selectedNode || mode === "run";
  $("workflow-delete-node").disabled = !has || !selectedNode || mode === "run";
  $("workflow-delete-edge").disabled = !has || !selectedEdge || mode === "run";
  $("workflow-add-node").disabled = !has || mode === "run";
  for (const id of ["workflow-zoom-in", "workflow-zoom-out", "workflow-zoom-reset", "workflow-fit"]) {
    $(id).disabled = !has;
  }
}

/// Client validation errors, server errors and the dirty-switch confirmation
/// all render in the same place, near the toolbar (U12) -- offending nodes
/// get highlighted on the canvas too.
function renderProblems() {
  const box = $("workflow-error");
  const highlighted = new Set();
  if (pendingDiscard) {
    box.innerHTML = `You have unsaved changes. <button type="button" class="btn" id="wf-discard">Discard and continue</button> <button type="button" class="btn" id="wf-keep-editing">Keep editing</button>`;
    box.hidden = false;
    $("wf-discard").onclick = () => { const go = pendingDiscard; pendingDiscard = null; renderProblems(); go(); };
    $("wf-keep-editing").onclick = () => { pendingDiscard = null; renderProblems(); };
  } else {
    const messages = [...clientErrors, ...(serverError ? [{ message: serverError }] : [])];
    for (const error of clientErrors) if (error.nodeId) highlighted.add(error.nodeId);
    box.innerHTML = messages.map(error => esc(error.message)).join("<br>");
    box.hidden = messages.length === 0;
  }
  for (const el of $("workflow-nodes")?.querySelectorAll("[data-node]") || []) {
    el.classList.toggle("wf-invalid", highlighted.has(el.dataset.node));
  }
  $("workflow-notice").hidden = !notice;
  $("workflow-notice").textContent = notice || "";
}

function showServerError(error) { serverError = error?.message || String(error); renderProblems(); }

function setBusy(busy) {
  saving = busy;
  renderButtons();
}

// -------------------------------------------------------------- run list

async function loadRuns(selectRunId) {
  if (!current?.id) { runs = []; renderRunsList(); return; }
  try {
    runs = (await api(`/api/workflows/${current.id}/runs?limit=20`)).runs;
    const picked = selectRunId ? runs.find(run => run.id === selectRunId) : runs[0];
    if (picked) { currentRun = picked; if (selectRunId) mode = "run"; }
    renderEditor();
  } catch (error) { showServerError(error); }
}

function renderRunsList() {
  $("workflow-runs").innerHTML = runs.length ? runs.map(run => `
    <button type="button" class="wf-run-item${currentRun?.id === run.id ? " on" : ""}" data-run="${esc(run.id)}">
      ${esc(run.status)} · r${esc(run.revision)}<span class="sub">${esc(new Date(run.created_at).toLocaleString())}</span>
    </button>`).join("") : `<div class="empty">No runs yet.</div>`;
  for (const button of $("workflow-runs").querySelectorAll("[data-run]")) {
    button.onclick = () => selectRun(button.dataset.run);
  }
}

function selectRun(id) {
  const run = runs.find(item => item.id === id);
  if (!run) return;
  currentRun = run;
  mode = "run";
  renderEditor();
  writeHash();
}

function renderRunStatusChip() {
  $("workflow-run-status").innerHTML = currentRun
    ? `run ${esc(currentRun.id.slice(0, 8))} · r${esc(currentRun.revision)} · <span class="wf-badge ${nodeStatusClass(currentRun.status)}">${esc(currentRun.status)}</span>`
    : "not run yet";
}

// ---------------------------------------------------------------- actions

async function save() {
  readEditor();
  current.name = current.name.trim();
  const errors = validate(current);
  clientErrors = errors; serverError = null;
  if (errors.length) { renderCanvas(); renderProblems(); return; }
  setBusy(true);
  const draft = { name: current.name, description: current.description, scope: current.scope, nodes: current.nodes, edges: current.edges };
  try {
    const answer = current.id ? await api(`/api/workflows/${current.id}`, { method: "PATCH", body: JSON.stringify(draft) })
      : await api("/api/workflows", { method: "POST", body: JSON.stringify(draft) });
    current = structuredClone(answer.workflow);
    const index = workflows.findIndex(workflow => workflow.id === current.id);
    if (index < 0) workflows.unshift(structuredClone(current)); else workflows[index] = structuredClone(current);
    clearDirty(); notice = null;
    renderWorkflows(); writeHash(true);
  } catch (error) { showServerError(error); }
  finally { setBusy(false); }
}

async function run() {
  if (!current?.id) return;
  setBusy(true); serverError = null;
  try {
    currentRun = (await api(`/api/workflows/${current.id}/run`, { method: "POST" })).run;
    mode = "run";
    await loadRuns(currentRun.id);
    writeHash();
  } catch (error) { showServerError(error); }
  finally { setBusy(false); }
}

async function cancel() {
  if (!currentRun || currentRun.status !== "running") return;
  setBusy(true);
  try { currentRun = (await api(`/api/workflow-runs/${currentRun.id}/cancel`, { method: "POST" })).run; renderEditor(); }
  catch (error) { showServerError(error); }
  finally { setBusy(false); }
}

async function removeWorkflow() {
  if (!current?.id || !confirm(`Delete workflow "${current.name}"? Run history and spawned tasks remain.`)) return;
  const id = current.id;
  try {
    await api(`/api/workflows/${id}`, { method: "DELETE" });
    workflows = workflows.filter(workflow => workflow.id !== id);
    closeWorkflow();
    renderWorkflows(); writeHash();
  } catch (error) { showServerError(error); }
}

function addNode() {
  if (mode === "run") return;
  const near = selectedNode && current.nodes.find(item => item.id === selectedNode);
  const at = freePosition(current.nodes, near ? near.position.x + 40 : 80, near ? near.position.y + 40 : 80);
  const node = newTaskNode(current.scope, at.x, at.y);
  current.nodes.push(node);
  selectedNode = node.id; selectedEdge = null;
  markDirty();
  renderEditor();
}

function duplicate() {
  if (mode === "run" || !selectedNode) return;
  const copy = duplicateNode(current.nodes, selectedNode);
  if (!copy) return;
  current.nodes.push(copy);
  selectedNode = copy.id;
  markDirty();
  renderEditor();
}

/// R2: deletes `id` -- defaulting to `selectedNode` for the toolbar button,
/// which has no card of its own to target -- rather than always trusting
/// `selectedNode`, which a keyboard user's focus can otherwise outrun (Tab
/// moves focus without necessarily going through `selectedNode` first).
function deleteNode(id = selectedNode) {
  if (mode === "run" || !id) return;
  current.nodes = current.nodes.filter(node => node.id !== id);
  current.edges = current.edges.filter(edge => edge.from !== id && edge.to !== id);
  if (selectedNode === id) selectedNode = null;
  selectedEdge = null; connectFrom = null;
  markDirty();
  renderEditor();
  $("workflow-canvas").focus();
}

function deleteEdge(id) {
  if (mode === "run") return;
  current.edges = current.edges.filter(edge => edge.id !== id);
  if (selectedEdge === id) selectedEdge = null;
  markDirty();
  renderEditor();
}

function zoomBy(factor) {
  if (!current) return;
  view.zoom = Math.max(.25, Math.min(2, view.zoom * factor));
  renderCanvas();
}

function zoomReset() {
  if (!current) return;
  view.zoom = 1;
  renderCanvas();
}

function fitToContent() {
  if (!current) return;
  const graph = activeGraph();
  const canvas = $("workflow-canvas");
  view = fitView(graph.nodes, canvas.clientWidth || 800, canvas.clientHeight || 500);
  renderCanvas();
}

function setMode(next) {
  if (next === mode || !current) return;
  mode = next;
  selectedNode = null; selectedEdge = null; connectFrom = null;
  inspectorRenderedFor = null;
  renderEditor();
  writeHash();
}

// ------------------------------------------------------------------ events

export function acceptWorkflowEvent(event) {
  if (event.type === "workflow_run_updated") {
    applyRunEvent(event.run);
    return;
  }
  const wasOpen = event.type === "workflow_deleted" && current?.id === event.id;
  const next = applyWorkflowEvent({ workflows, current, currentRun, dirty }, event);
  workflows = next.workflows;
  if (next.notice) notice = next.notice;
  const currentChanged = next.current !== current;
  current = next.current;
  currentRun = next.currentRun;
  if (wasOpen) {
    // R3/R4: someone else's delete (or this tab's own delete, echoed back)
    // closes the workflow exactly as `removeWorkflow` does -- clearing
    // "Recent runs" and correcting the route tail, not just `current`.
    closeWorkflow();
    mode = "design";
    writeHash(true);
  }
  renderWorkflows();
  if (currentChanged) renderProblems();
}

function applyRunEvent(run) {
  if (current?.id !== run.workflow_id) return;
  const index = runs.findIndex(item => item.id === run.id);
  if (index < 0) runs.unshift(run); else runs[index] = run;
  renderRunsList();
  if (!currentRun || currentRun.id === run.id) {
    const firstSighting = !currentRun;
    currentRun = run;
    if (mode === "run") {
      if (firstSighting) renderCanvas(); else paintStatuses();
      renderRunPanel();
      for (const button of $("workflow-mode").querySelectorAll("[data-mode]")) {
        if (button.dataset.mode === "run") button.disabled = false;
      }
    } else {
      renderRunStatusChip();
    }
    renderButtons();
  } else {
    renderRunStatusChip();
  }
}

export function wireWorkflows() {
  if (wired) return; wired = true;
  $("workflow-new").onclick = newWorkflow;
  $("workflow-save").onclick = save;
  $("workflow-run").onclick = run;
  $("workflow-cancel").onclick = cancel;
  $("workflow-delete").onclick = removeWorkflow;
  $("workflow-add-node").onclick = addNode;
  $("workflow-duplicate").onclick = duplicate;
  $("workflow-delete-node").onclick = () => deleteNode();
  $("workflow-delete-edge").onclick = () => selectedEdge && deleteEdge(selectedEdge);
  $("workflow-connect").onclick = () => { if (selectedNode && mode !== "run") { connectFrom = selectedNode; renderCanvas(); } };
  $("workflow-zoom-in").onclick = () => zoomBy(1.2);
  $("workflow-zoom-out").onclick = () => zoomBy(1 / 1.2);
  $("workflow-zoom-reset").onclick = zoomReset;
  $("workflow-fit").onclick = fitToContent;
  for (const button of $("workflow-mode").querySelectorAll("[data-mode]")) {
    button.onclick = () => setMode(button.dataset.mode);
  }
  for (const id of ["workflow-name", "workflow-description", "workflow-node-title", "workflow-node-instructions", "workflow-node-agent", "workflow-node-worktree", "workflow-node-estimate", "workflow-node-ack", "workflow-node-timeout", "workflow-node-blocked"]) {
    $(id).oninput = () => { readEditor(); markDirty(); renderCanvas(); renderSummary(); };
  }
  const canvas = $("workflow-canvas");
  canvas.onkeydown = event => {
    if (event.key === "Escape") { cancelConnect(); if (selectedEdge) { selectedEdge = null; paintStatuses(); } }
    if ((event.key === "Delete" || event.key === "Backspace") && selectedEdge) { event.preventDefault(); deleteEdge(selectedEdge); }
  };
  canvas.onwheel = event => {
    event.preventDefault();
    view.zoom = Math.max(.25, Math.min(2, view.zoom * (event.deltaY > 0 ? .9 : 1.1)));
    renderCanvas();
  };
  canvas.onpointerdown = event => {
    if (event.target.closest?.(".workflow-node, [data-edge]")) return;
    const start = { x: event.clientX, y: event.clientY, vx: view.x, vy: view.y };
    let dragging = false;
    canvas.setPointerCapture(event.pointerId);
    canvas.onpointermove = move => {
      const at = { x: move.clientX, y: move.clientY };
      if (!dragging && !isDrag(start, at)) return; // R1: the same incidental pointermove, on the background
      dragging = true;
      view.x = start.vx + at.x - start.x; view.y = start.vy + at.y - start.y; renderCanvas();
    };
    canvas.onpointerup = () => {
      canvas.onpointermove = null;
      if (!dragging) { selectedEdge = null; paintStatuses(); $("workflow-hint").textContent = edgeHint(); }
    };
  };
}
