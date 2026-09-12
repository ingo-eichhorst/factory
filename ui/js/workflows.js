//! Process-level workflow list, canvas and task-node inspector.

import { $, api, esc, state, statusBadge } from "./core.js";
import { inScope, writeHash } from "./scopes.js";
import { openTask } from "./tasks.js";
import { topologicalSummary } from "./workflow-graph.js";

let workflows = [];
let current = null;
let currentRun = null;
let selectedNode = null;
let connectFrom = null;
let view = { x: 40, y: 40, zoom: 1 };
let wired = false;

const uid = (prefix) => `${prefix}-${globalThis.crypto?.randomUUID?.() || Math.random().toString(36).slice(2)}`;
const number = (id) => { const value = $(id).value.trim(); return value ? Number(value) : null; };

function blank(scope) {
  return { id: null, name: "Untitled workflow", description: "", scope, revision: null, nodes: [], edges: [] };
}

function taskNode(x = 80, y = 80, source = null) {
  const task = source ? structuredClone(source.task) : {
    title: "New task", instructions: "", scope: current.scope, agent: null,
    worktree: true, estimate_seconds: null, ack_timeout_seconds: null, timeout_seconds: null,
    blocked_timeout_seconds: null,
  };
  return { id: uid("node"), position: { x, y }, kind: "task", task };
}

export function workflowTail() { return current?.id ? [current.id] : []; }

export function readWorkflowTail([id]) {
  if (!id) return;
  const found = workflows.find(workflow => workflow.id === id);
  if (found) open(found, false);
  else loadWorkflows(id);
}

export async function loadWorkflows(wanted) {
  setBusy(true);
  try {
    workflows = (await api("/api/workflows")).workflows;
    if (wanted) {
      const found = workflows.find(workflow => workflow.id === wanted);
      if (found) open(found, false);
    } else if (current?.id) {
      const fresh = workflows.find(workflow => workflow.id === current.id);
      if (fresh) current = structuredClone(fresh);
    }
    renderWorkflows();
  } catch (error) { showError(error); }
  finally { setBusy(false); }
}

export function renderWorkflows() {
  if (current && !inScope(current.scope)) {
    current = null;
    currentRun = null;
    selectedNode = null;
  }
  const visible = workflows.filter(workflow => inScope(workflow.scope));
  $("workflow-list").innerHTML = visible.length ? visible.map(workflow => `
    <button class="workflow-list-item${current?.id === workflow.id ? " on" : ""}" data-id="${esc(workflow.id)}">
      <strong>${esc(workflow.name)}</strong><span>${esc(workflow.scope)} · r${workflow.revision}</span>
    </button>`).join("") : `<div class="empty">No workflows in this scope.</div>`;
  for (const button of $("workflow-list").querySelectorAll("[data-id]")) {
    button.onclick = () => open(workflows.find(workflow => workflow.id === button.dataset.id));
  }
  $("workflow-new").disabled = state.scope === null;
  $("workflow-new").title = state.scope === null ? "Select one owning scope first" : "";
  renderEditor();
}

function open(workflow, navigate = true) {
  current = structuredClone(workflow);
  currentRun = null;
  selectedNode = null;
  connectFrom = null;
  view = { x: 40, y: 40, zoom: 1 };
  renderWorkflows();
  loadLatestRun();
  if (navigate) writeHash();
}

function newWorkflow() {
  if (!state.scope) return;
  current = blank(state.scope);
  current.nodes.push(taskNode());
  selectedNode = current.nodes[0].id;
  currentRun = null;
  renderWorkflows();
  writeHash();
}

function renderEditor() {
  const has = !!current;
  for (const id of ["workflow-save", "workflow-run", "workflow-delete", "workflow-add-node", "workflow-connect", "workflow-duplicate", "workflow-delete-node"]) {
    $(id).disabled = !has;
  }
  $("workflow-delete").disabled = !current?.id;
  $("workflow-connect").disabled = !selectedNode;
  $("workflow-duplicate").disabled = !selectedNode;
  $("workflow-delete-node").disabled = !selectedNode;
  $("workflow-run").disabled = !current?.id || currentRun?.status === "running";
  $("workflow-cancel").disabled = currentRun?.status !== "running";
  if (!has) {
    $("workflow-name").value = ""; $("workflow-description").value = ""; $("workflow-scope").value = "";
    $("workflow-node-fields").hidden = true; $("workflow-nodes").innerHTML = ""; $("workflow-edges").innerHTML = "";
    $("workflow-summary").innerHTML = ""; $("workflow-revision").textContent = "";
    $("workflow-run-status").textContent = "";
    return;
  }
  $("workflow-name").value = current.name;
  $("workflow-description").value = current.description || "";
  $("workflow-scope").value = current.scope;
  $("workflow-revision").textContent = current.revision ? `definition revision ${current.revision}` : "not saved";
  $("workflow-run-status").innerHTML = currentRun
    ? `run ${esc(currentRun.id.slice(0, 8))} · revision ${currentRun.revision} · ${statusBadge(currentRun.status)}` : "not run yet";
  renderCanvas();
  renderInspector();
  renderSummary();
}

function runNode(id) { return currentRun?.nodes?.find(node => node.node_id === id); }

function renderCanvas() {
  $("workflow-world").style.transform = `translate(${view.x}px,${view.y}px) scale(${view.zoom})`;
  $("workflow-zoom").textContent = `${Math.round(view.zoom * 100)}%`;
  $("workflow-edges").setAttribute("viewBox", "0 0 2400 1600");
  $("workflow-edges").innerHTML = `<defs><marker id="workflow-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" /></marker></defs>` +
    current.edges.map(edge => {
      const from = current.nodes.find(node => node.id === edge.from), to = current.nodes.find(node => node.id === edge.to);
      if (!from || !to) return "";
      const x1 = from.position.x + 184, y1 = from.position.y + 42, x2 = to.position.x, y2 = to.position.y + 42;
      const bend = Math.max(45, Math.abs(x2 - x1) * .45);
      return `<path class="workflow-edge" data-id="${esc(edge.id)}" d="M${x1},${y1} C${x1 + bend},${y1} ${x2 - bend},${y2} ${x2},${y2}" marker-end="url(#workflow-arrow)"/>`;
    }).join("");
  $("workflow-nodes").innerHTML = current.nodes.map(node => {
    const execution = runNode(node.id);
    const status = execution?.status || "unstarted";
    const connect = connectFrom === node.id ? " connecting" : "";
    return `<button class="workflow-node s-${esc(status)}${selectedNode === node.id ? " selected" : ""}${connect}" data-node="${esc(node.id)}" style="left:${node.position.x}px;top:${node.position.y}px">
      <span class="workflow-node-kind">TASK</span><strong>${esc(node.task.title || "Untitled task")}</strong>
      <span>${statusBadge(status)} ${esc(node.task.agent || "default agent")}</span>
      ${execution?.task_id ? `<i data-task="${esc(execution.task_id)}" title="Open spawned task">open task ↗</i>` : ""}
    </button>`;
  }).join("");
  for (const node of $("workflow-nodes").querySelectorAll("[data-node]")) wireNode(node);
}

function select(id) {
  if (connectFrom && connectFrom !== id) {
    if (!current.edges.some(edge => edge.from === connectFrom && edge.to === id)) {
      current.edges.push({ id: uid("edge"), from: connectFrom, to: id });
    }
    connectFrom = null;
  }
  selectedNode = id;
  renderEditor();
}

function wireNode(element) {
  element.onclick = event => {
    const task = event.target.closest("[data-task]");
    if (task) { event.stopPropagation(); openTask(task.dataset.task); return; }
    select(element.dataset.node);
  };
  element.onkeydown = event => {
    if (event.key === "Delete" || event.key === "Backspace") { event.preventDefault(); deleteNode(); return; }
    const delta = event.shiftKey ? 20 : 5;
    const move = { ArrowLeft: [-delta, 0], ArrowRight: [delta, 0], ArrowUp: [0, -delta], ArrowDown: [0, delta] }[event.key];
    if (!move) return;
    event.preventDefault();
    const node = current.nodes.find(item => item.id === element.dataset.node);
    node.position.x += move[0]; node.position.y += move[1]; renderCanvas(); renderSummary();
    $("workflow-nodes").querySelector(`[data-node="${CSS.escape(node.id)}"]`)?.focus();
  };
  element.onpointerdown = event => {
    if (event.button !== 0 || event.target.closest("[data-task]")) return;
    const node = current.nodes.find(item => item.id === element.dataset.node);
    const start = { x: event.clientX, y: event.clientY, nx: node.position.x, ny: node.position.y };
    element.setPointerCapture(event.pointerId);
    element.onpointermove = move => {
      node.position.x = Math.round(start.nx + (move.clientX - start.x) / view.zoom);
      node.position.y = Math.round(start.ny + (move.clientY - start.y) / view.zoom);
      element.style.left = `${node.position.x}px`;
      element.style.top = `${node.position.y}px`;
    };
    element.onpointerup = () => { element.onpointermove = null; renderSummary(); };
  };
}

function renderInspector() {
  const node = current.nodes.find(item => item.id === selectedNode);
  $("workflow-node-fields").hidden = !node;
  if (!node) return;
  const agents = state.scopes.find(scope => scope.name === current.scope)?.agents || [];
  $("workflow-node-agent").innerHTML = `<option value="">Default agent</option>` + agents.map(agent => `<option value="${esc(agent.name)}">${esc(agent.name)}</option>`).join("");
  $("workflow-node-title").value = node.task.title || "";
  $("workflow-node-instructions").value = node.task.instructions || "";
  $("workflow-node-agent").value = node.task.agent || "";
  $("workflow-node-worktree").checked = node.task.worktree !== false;
  $("workflow-node-estimate").value = node.task.estimate_seconds || "";
  $("workflow-node-ack").value = node.task.ack_timeout_seconds || "";
  $("workflow-node-timeout").value = node.task.timeout_seconds || "";
  $("workflow-node-blocked").value = node.task.blocked_timeout_seconds || "";
}

function renderSummary() {
  const summary = topologicalSummary(current);
  $("workflow-summary").innerHTML = summary.error ? `<li class="err">${esc(summary.error)}</li>` : summary.nodes.map(item => {
    const node = current.nodes.find(node => node.id === item.id), execution = runNode(item.id);
    return `<li><button data-summary-node="${esc(item.id)}">${esc(node.task.title)}</button> ${statusBadge(execution?.status || "unstarted")}<span> after ${item.after.length ? item.after.map(id => esc(current.nodes.find(node => node.id === id)?.task.title || id)).join(", ") : "start"}</span></li>`;
  }).join("");
  for (const button of $("workflow-summary").querySelectorAll("[data-summary-node]")) button.onclick = () => select(button.dataset.summaryNode);
}

function readEditor() {
  current.name = $("workflow-name").value.trim(); current.description = $("workflow-description").value;
  const node = current.nodes.find(item => item.id === selectedNode);
  if (node) {
    node.task.title = $("workflow-node-title").value.trim();
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

async function save() {
  readEditor(); showError(null); setBusy(true);
  const draft = { name: current.name, description: current.description, scope: current.scope, nodes: current.nodes, edges: current.edges };
  try {
    const answer = current.id ? await api(`/api/workflows/${current.id}`, { method: "PATCH", body: JSON.stringify(draft) })
      : await api("/api/workflows", { method: "POST", body: JSON.stringify(draft) });
    current = structuredClone(answer.workflow);
    const index = workflows.findIndex(workflow => workflow.id === current.id);
    if (index < 0) workflows.unshift(structuredClone(current)); else workflows[index] = structuredClone(current);
    renderWorkflows(); writeHash(true);
  } catch (error) { showError(error); }
  finally { setBusy(false); }
}

async function run() {
  if (!current?.id) return;
  setBusy(true); showError(null);
  try { currentRun = (await api(`/api/workflows/${current.id}/run`, { method: "POST" })).run; renderEditor(); }
  catch (error) { showError(error); }
  finally { setBusy(false); }
}

async function cancel() {
  if (!currentRun || currentRun.status !== "running") return;
  setBusy(true);
  try { currentRun = (await api(`/api/workflow-runs/${currentRun.id}/cancel`, { method: "POST" })).run; renderEditor(); }
  catch (error) { showError(error); }
  finally { setBusy(false); }
}

async function removeWorkflow() {
  if (!current?.id || !confirm(`Delete workflow “${current.name}”? Run history and spawned tasks remain.`)) return;
  try {
    await api(`/api/workflows/${current.id}`, { method: "DELETE" });
    workflows = workflows.filter(workflow => workflow.id !== current.id); current = null; currentRun = null; renderWorkflows(); writeHash();
  } catch (error) { showError(error); }
}

async function loadLatestRun() {
  if (!current?.id) return;
  try { currentRun = (await api(`/api/workflows/${current.id}/runs?limit=1`)).runs[0] || null; renderEditor(); }
  catch (error) { showError(error); }
}

function addNode() { const node = taskNode(80 + current.nodes.length * 36, 80 + current.nodes.length * 28); current.nodes.push(node); selectedNode = node.id; renderEditor(); }
function duplicateNode() { const node = current.nodes.find(item => item.id === selectedNode); if (!node) return; const copy = taskNode(node.position.x + 32, node.position.y + 32, node); copy.task.title += " copy"; current.nodes.push(copy); selectedNode = copy.id; renderEditor(); }
function deleteNode() { if (!selectedNode) return; current.nodes = current.nodes.filter(node => node.id !== selectedNode); current.edges = current.edges.filter(edge => edge.from !== selectedNode && edge.to !== selectedNode); selectedNode = null; connectFrom = null; renderEditor(); }
function showError(error) { $("workflow-error").textContent = error ? error.message || String(error) : ""; }
function setBusy(busy) {
  for (const id of ["workflow-save", "workflow-run", "workflow-cancel"]) {
    if (!$(id)) continue;
    $(id).classList.toggle("loading", busy);
    if (busy) $(id).disabled = true;
  }
  if (!busy) renderEditor();
}

export function acceptWorkflowEvent(event) {
  if (event.type === "workflow_created" || event.type === "workflow_updated") {
    const definition = event.workflow, index = workflows.findIndex(item => item.id === definition.id);
    if (index < 0) workflows.unshift(definition); else workflows[index] = definition;
    if (current?.id === definition.id) current = structuredClone(definition);
    renderWorkflows();
  } else if (event.type === "workflow_deleted") {
    workflows = workflows.filter(item => item.id !== event.id);
    if (current?.id === event.id) { current = null; currentRun = null; }
    renderWorkflows();
  } else if (event.type === "workflow_run_updated" && current?.id === event.run.workflow_id) {
    currentRun = event.run; renderEditor();
  }
}

export function wireWorkflows() {
  if (wired) return; wired = true;
  $("workflow-new").onclick = newWorkflow; $("workflow-save").onclick = save; $("workflow-run").onclick = run;
  $("workflow-cancel").onclick = cancel; $("workflow-delete").onclick = removeWorkflow;
  $("workflow-add-node").onclick = addNode; $("workflow-duplicate").onclick = duplicateNode; $("workflow-delete-node").onclick = deleteNode;
  $("workflow-connect").onclick = () => { if (selectedNode) { connectFrom = selectedNode; renderCanvas(); } };
  for (const id of ["workflow-name", "workflow-description", "workflow-node-title", "workflow-node-instructions", "workflow-node-agent", "workflow-node-worktree", "workflow-node-estimate", "workflow-node-ack", "workflow-node-timeout", "workflow-node-blocked"]) {
    $(id).oninput = () => { readEditor(); renderCanvas(); renderSummary(); };
  }
  const canvas = $("workflow-canvas");
  canvas.onwheel = event => { event.preventDefault(); view.zoom = Math.max(.45, Math.min(1.8, view.zoom * (event.deltaY > 0 ? .9 : 1.1))); renderCanvas(); };
  canvas.onpointerdown = event => {
    if (event.target.closest?.(".workflow-node")) return;
    const start = { x: event.clientX, y: event.clientY, vx: view.x, vy: view.y };
    canvas.setPointerCapture(event.pointerId);
    canvas.onpointermove = move => { view.x = start.vx + move.clientX - start.x; view.y = start.vy + move.clientY - start.y; renderCanvas(); };
    canvas.onpointerup = () => { canvas.onpointermove = null; };
  };
}
