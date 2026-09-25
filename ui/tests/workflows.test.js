import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  connectionError,
  edgeStatusClass,
  fitView,
  freePosition,
  isDrag,
  nodeStatusClass,
  NODE_STATUSES,
  reachable,
  removeEdge,
  removeNode,
  rootIds,
  topologicalSummary,
} from "../js/workflow-graph.js";
import {
  addEdge,
  applyWorkflowEvent,
  duplicateNode,
  editorButtons,
  newTaskNode,
  openTaskAction,
  readWorkflowRouteTail,
  validate,
  workflowRouteTail,
} from "../js/workflow-model.js";

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const app = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");

test("Workflows is a Process peer of Tasks with an accessible canvas and summary", () => {
  // Intake (`#119`) sits between them -- the queue in front of the line --
  // and `intake.test.js` pins that half.
  assert.match(page, /id="tab-intake"[^>]*>Intake<\/button>\s*<button id="tab-workflows"[^>]*>Workflows<\/button>/);
  assert.match(page, /id="workflow-canvas"[^>]*tabindex="0"/);
  assert.match(page, /id="workflow-summary"/);
  // Not anchored at the closing bracket: Operations follows (`#106`), and
  // `operations.test.js` pins the whole row.
  assert.match(app, /proc: \["tasks", "intake", "workflows"/);
});

// ------------------------------------------------------------------ R1: drag

test("isDrag: a zero-distance pointermove (Chrome's own, after setPointerCapture) is not a drag", () => {
  assert.equal(isDrag({ x: 100, y: 100 }, { x: 100, y: 100 }), false);
});

test("isDrag: movement inside the threshold is still a click, not a drag", () => {
  assert.equal(isDrag({ x: 100, y: 100 }, { x: 102, y: 100 }, 3), false);
});

test("isDrag: movement past the threshold is a drag", () => {
  assert.equal(isDrag({ x: 100, y: 100 }, { x: 105, y: 100 }, 3), true);
});

test("isDrag: even a zero threshold requires some actual movement", () => {
  assert.equal(isDrag({ x: 100, y: 100 }, { x: 100, y: 100 }, 0), false);
  assert.equal(isDrag({ x: 100, y: 100 }, { x: 100.5, y: 100 }, 0), true);
});

// ------------------------------------------------------------- graph editing

function node(id, x = 0, y = 0, title = id) {
  return { id, position: { x, y }, kind: "task", task: { title } };
}

test("a new task node gets an id nothing else has and Factory's ordinary defaults", () => {
  const a = newTaskNode("demo", 10, 20);
  const b = newTaskNode("demo", 10, 20);
  assert.notEqual(a.id, b.id);
  assert.equal(a.task.scope, "demo");
  assert.equal(a.task.worktree, true);
  assert.equal(a.task.schedule, undefined);
  assert.deepEqual(a.position, { x: 10, y: 20 });
});

test("duplicating a node gets a new id and an offset, non-overlapping position", () => {
  const nodes = [node("a", 100, 100)];
  const copy = duplicateNode(nodes, "a");
  assert.notEqual(copy.id, "a");
  assert.equal(copy.task.title, "a copy");
  assert.notDeepEqual(copy.position, nodes[0].position);
  // The copy must not land on top of the original.
  const overlaps =
    Math.abs(copy.position.x - nodes[0].position.x) < 184 &&
    Math.abs(copy.position.y - nodes[0].position.y) < 84;
  assert.equal(overlaps, false);
});

test("duplicating a node that no longer exists yields nothing", () => {
  assert.equal(duplicateNode([node("a")], "ghost"), null);
});

test("free position walks clear of every existing card, not just the nearest one", () => {
  const nodes = [node("a", 0, 0), node("b", 36, 28), node("c", 72, 56)];
  const at = freePosition(nodes, 0, 0);
  for (const n of nodes) {
    const overlaps = Math.abs(n.position.x - at.x) < 184 && Math.abs(n.position.y - at.y) < 84;
    assert.equal(overlaps, false, `new position ${JSON.stringify(at)} overlaps ${n.id}`);
  }
});

test("deleting a node removes its incident edges and nothing else", () => {
  const nodes = [node("a"), node("b"), node("c")];
  const edges = [{ id: "ab", from: "a", to: "b" }, { id: "bc", from: "b", to: "c" }];
  const result = removeNode(nodes, edges, "b");
  assert.deepEqual(result.nodes.map(n => n.id), ["a", "c"]);
  assert.deepEqual(result.edges, []);
});

test("deleting a node not touched by any edge leaves the rest alone", () => {
  const nodes = [node("a"), node("b")];
  const edges = [{ id: "ab", from: "a", to: "b" }];
  const result = removeNode(nodes, edges, "a");
  assert.deepEqual(result.nodes.map(n => n.id), ["b"]);
  assert.deepEqual(result.edges, []);
});

test("R2: delete targets exactly the given id, not a separately-tracked selection", () => {
  // Regression for the workflows.js bug: `selectedNode` can lag behind
  // keyboard focus (select "merge" via a button, Tab to "branch B", press
  // Delete). `removeNode` takes the id to remove as an explicit argument --
  // this pins that a caller acting on the *focused* card's id, whatever
  // `selectedNode` happens to hold, is the contract to keep.
  const nodes = [node("merge"), node("branchB")];
  const focusedId = "branchB"; // not "merge", which some other state calls "selected"
  const result = removeNode(nodes, [], focusedId);
  assert.deepEqual(result.nodes.map(n => n.id), ["merge"]);
});

test("connecting a node to itself is refused, naming the node", () => {
  const nodes = [node("a", 0, 0, "Root")];
  const error = connectionError(nodes, [], "a", "a");
  assert.match(error, /"Root"/);
  assert.match(error, /itself/);
});

test("a duplicate link is refused, naming both nodes", () => {
  const nodes = [node("a", 0, 0, "Root"), node("b", 0, 0, "Left")];
  const edges = [{ id: "ab", from: "a", to: "b" }];
  const error = connectionError(nodes, edges, "a", "b");
  assert.match(error, /"Root"/);
  assert.match(error, /"Left"/);
  assert.match(error, /already links/);
});

test("a link that would close a cycle is refused, naming both nodes", () => {
  const nodes = [node("a", 0, 0, "Root"), node("b", 0, 0, "Left")];
  const edges = [{ id: "ab", from: "a", to: "b" }];
  const error = connectionError(nodes, edges, "b", "a");
  assert.match(error, /"Root"/);
  assert.match(error, /"Left"/);
  assert.match(error, /cycle/);
});

test("a link that does not close a cycle is accepted", () => {
  const nodes = [node("a"), node("b"), node("c")];
  const edges = [{ id: "ab", from: "a", to: "b" }];
  assert.equal(connectionError(nodes, edges, "b", "c"), null);
  assert.equal(reachable(edges, "b", "a"), false);
  assert.equal(reachable(edges, "a", "b"), true);
});

test("deleting an edge removes only that edge", () => {
  const edges = [{ id: "ab", from: "a", to: "b" }, { id: "bc", from: "b", to: "c" }];
  assert.deepEqual(removeEdge(edges, "ab"), [{ id: "bc", from: "b", to: "c" }]);
});

test("addEdge appends a directed edge with the given id", () => {
  const edges = addEdge([], "e1", "a", "b");
  assert.deepEqual(edges, [{ id: "e1", from: "a", to: "b" }]);
});

// -------------------------------------------------------- root/topology

test("root detection finds every node with no incoming edge", () => {
  const nodes = [node("a"), node("b"), node("c")];
  const edges = [{ id: "ab", from: "a", to: "b" }];
  assert.deepEqual(rootIds(nodes, edges).sort(), ["a", "c"]);
});

test("the textual order represents fan-out and fan-in", () => {
  const workflow = {
    nodes: ["a", "b", "c", "d"].map(id => node(id)),
    edges: [
      { from: "a", to: "b" }, { from: "a", to: "c" },
      { from: "b", to: "d" }, { from: "c", to: "d" },
    ],
  };
  const summary = topologicalSummary(workflow);
  assert.equal(summary.error, null);
  assert.equal(summary.nodes[0].id, "a");
  assert.equal(summary.nodes.at(-1).id, "d");
  assert.deepEqual(summary.nodes.at(-1).after.sort(), ["b", "c"]);
});

test("cycles are reported with the node titles, not just their ids", () => {
  const summary = topologicalSummary({
    nodes: [node("a", 0, 0, "Root"), node("b", 0, 0, "Left")],
    edges: [{ from: "a", to: "b" }, { from: "b", to: "a" }],
  });
  assert.match(summary.error, /cycle/);
  assert.match(summary.error, /"Root"/);
  assert.match(summary.error, /"Left"/);
});

// ---------------------------------------------------------- status overlays

test("every node status (the issue's 8 visual categories, 9 enum values since pending and dispatching share one) maps to its own class, and an unknown one is neutral", () => {
  assert.equal(NODE_STATUSES.length, 9, "9 WorkflowNodeStatus variants");
  const classes = new Set(NODE_STATUSES.map(nodeStatusClass));
  assert.equal(classes.size, NODE_STATUSES.length, "no two statuses share a class");
  assert.equal(nodeStatusClass("done"), "wf-s-done");
  assert.equal(nodeStatusClass("skipped"), "wf-s-skipped");
  assert.equal(nodeStatusClass(undefined), "wf-s-unstarted");
  assert.equal(nodeStatusClass("nonsense"), "wf-s-unstarted");
});

test("an edge reflects progress: a done parent draws in the run colour, anything else does not", () => {
  assert.equal(edgeStatusClass("done"), "wf-edge-done");
  for (const status of NODE_STATUSES.filter(s => s !== "done")) {
    assert.equal(edgeStatusClass(status), "");
  }
});

test("fit-to-content frames every node inside the viewport", () => {
  const nodes = [node("a", 0, 0), node("b", 800, 400)];
  const view = fitView(nodes, 1000, 700);
  assert.ok(view.zoom > 0 && view.zoom <= 1.5);
  assert.ok(Number.isFinite(view.x) && Number.isFinite(view.y));
});

test("fit-to-content on an empty graph is the canvas's ordinary opening view", () => {
  assert.deepEqual(fitView([], 1000, 700), { x: 40, y: 40, zoom: 1 });
});

// -------------------------------------------------------------- validation

function workflow(nodes, edges = []) {
  return { name: "pipeline", scope: "demo", nodes, edges };
}

test("an empty workflow needs at least one node", () => {
  const errors = validate(workflow([]));
  assert.ok(errors.some(e => /at least one/.test(e.message)));
});

test("R8: an empty title is refused with a human message, keyed to the highlighted node", () => {
  const errors = validate(workflow([node("a", 0, 0, "")]));
  const error = errors.find(e => e.nodeId === "a");
  assert.ok(error, "the offending node is named by id, for renderProblems to highlight");
  assert.match(error.message, /no title/i);
  // Not the opaque `"" (node-...)` shape an empty-title node label would
  // otherwise have produced -- there is nothing useful to quote.
  assert.doesNotMatch(error.message, /""/);
});

test("a non-positive duration is refused, naming which one", () => {
  const n = node("a", 0, 0, "Deploy");
  n.task.timeout_seconds = 0;
  const errors = validate(workflow([n]));
  assert.ok(errors.some(e => /run timeout/.test(e.message) && /"Deploy"/.test(e.message)));
});

test("R8: the same duration message on an untitled node reads as a node, not an empty quote", () => {
  const n = node("a", 0, 0, "");
  n.task.timeout_seconds = 0;
  const errors = validate(workflow([n]));
  assert.ok(errors.some(e => /run timeout/.test(e.message) && /^a task node/.test(e.message)));
});

test("an edge to a node that does not exist is refused", () => {
  const errors = validate(workflow([node("a")], [{ id: "e1", from: "a", to: "ghost" }]));
  assert.ok(errors.some(e => /e1/.test(e.message)));
});

test("a cycle is refused with the same message the summary gives", () => {
  const errors = validate(
    workflow([node("a"), node("b")], [{ id: "ab", from: "a", to: "b" }, { id: "ba", from: "b", to: "a" }]),
  );
  assert.ok(errors.some(e => /cycle/.test(e.message)));
});

test("a well-formed workflow validates clean", () => {
  const errors = validate(workflow([node("a"), node("b")], [{ id: "ab", from: "a", to: "b" }]));
  assert.deepEqual(errors, []);
});

// --------------------------------------------------------------- buttons

test("button state: nothing open disables save and run", () => {
  const buttons = editorButtons({ hasWorkflow: false, hasId: false, dirty: false, saving: false, run: null, mode: "design" });
  assert.equal(buttons.save.disabled, true);
  assert.equal(buttons.run.disabled, true);
  assert.equal(buttons.cancel.disabled, true);
});

test("button state: an unsaved new workflow can be saved but not run", () => {
  const buttons = editorButtons({ hasWorkflow: true, hasId: false, dirty: false, saving: false, run: null, mode: "design" });
  assert.equal(buttons.save.disabled, false);
  assert.equal(buttons.run.disabled, true);
});

test("button state: dirty disables run with an explaining hint, but not save", () => {
  const buttons = editorButtons({ hasWorkflow: true, hasId: true, dirty: true, saving: false, run: null, mode: "design" });
  assert.equal(buttons.save.disabled, false);
  assert.equal(buttons.run.disabled, true);
  assert.match(buttons.run.hint, /saved revision/);
});

test("button state: saving shows loading and disables save", () => {
  const buttons = editorButtons({ hasWorkflow: true, hasId: true, dirty: false, saving: true, run: null, mode: "design" });
  assert.equal(buttons.save.disabled, true);
  assert.equal(buttons.save.loading, true);
});

test("button state: a running run disables run and enables cancel", () => {
  const buttons = editorButtons({
    hasWorkflow: true, hasId: true, dirty: false, saving: false, run: { status: "running" }, mode: "design",
  });
  assert.equal(buttons.run.disabled, true);
  assert.equal(buttons.cancel.disabled, false);
});

test("button state: a terminal run re-enables run and disables cancel", () => {
  const buttons = editorButtons({
    hasWorkflow: true, hasId: true, dirty: false, saving: false, run: { status: "done" }, mode: "design",
  });
  assert.equal(buttons.run.disabled, false);
  assert.equal(buttons.cancel.disabled, true);
});

test("button state: run mode disables run regardless of everything else", () => {
  const buttons = editorButtons({ hasWorkflow: true, hasId: true, dirty: false, saving: false, run: null, mode: "run" });
  assert.equal(buttons.run.disabled, true);
});

// ---------------------------------------------------------------- events

test("a run update for the open workflow updates the run in place", () => {
  const state = { workflows: [], current: { id: "w1" }, currentRun: null, dirty: false };
  const next = applyWorkflowEvent(state, { type: "workflow_run_updated", run: { id: "r1", workflow_id: "w1", status: "done" } });
  assert.equal(next.currentRun.id, "r1");
});

test("a run update for a different workflow is ignored", () => {
  const state = { workflows: [], current: { id: "w1" }, currentRun: null, dirty: false };
  const next = applyWorkflowEvent(state, { type: "workflow_run_updated", run: { id: "r1", workflow_id: "w2", status: "done" } });
  assert.equal(next.currentRun, null);
});

test("deleting the open workflow clears the selection", () => {
  const state = { workflows: [{ id: "w1" }], current: { id: "w1" }, currentRun: { id: "r1" }, dirty: false };
  const next = applyWorkflowEvent(state, { type: "workflow_deleted", id: "w1" });
  assert.equal(next.current, null);
  assert.equal(next.currentRun, null);
  assert.deepEqual(next.workflows, []);
});

test("deleting a workflow that is not open only trims the list", () => {
  const state = { workflows: [{ id: "w1" }, { id: "w2" }], current: { id: "w2" }, currentRun: null, dirty: false };
  const next = applyWorkflowEvent(state, { type: "workflow_deleted", id: "w1" });
  assert.equal(next.current.id, "w2");
  assert.deepEqual(next.workflows.map(w => w.id), ["w2"]);
});

test("workflow_updated while dirty sets a notice instead of clobbering the edit", () => {
  const state = { workflows: [{ id: "w1", revision: 1 }], current: { id: "w1", name: "mine, unsaved" }, currentRun: null, dirty: true };
  const next = applyWorkflowEvent(state, { type: "workflow_updated", workflow: { id: "w1", revision: 2, name: "someone else's" } });
  assert.equal(next.current.name, "mine, unsaved", "the open edit is not overwritten");
  assert.match(next.notice, /revision 2/);
});

test("workflow_updated while clean replaces the open definition", () => {
  const state = { workflows: [{ id: "w1", revision: 1 }], current: { id: "w1", name: "old" }, currentRun: null, dirty: false };
  const next = applyWorkflowEvent(state, { type: "workflow_updated", workflow: { id: "w1", revision: 2, name: "new" } });
  assert.equal(next.current.name, "new");
});

// --------------------------------------------------------------- routing

test("the route tail round-trips a workflow id alone", () => {
  const tail = workflowRouteTail("wf1", null);
  assert.deepEqual(tail, ["wf1"]);
  assert.deepEqual(readWorkflowRouteTail(tail), { id: "wf1", runId: null });
});

test("the route tail round-trips a workflow id and a selected run", () => {
  const tail = workflowRouteTail("wf1", "run1");
  assert.deepEqual(tail, ["wf1", "run", "run1"]);
  assert.deepEqual(readWorkflowRouteTail(tail), { id: "wf1", runId: "run1" });
});

test("an old link naming no run still reads back the workflow id", () => {
  assert.deepEqual(readWorkflowRouteTail(["wf1"]), { id: "wf1", runId: null });
});

test("an empty tail names no workflow", () => {
  assert.deepEqual(readWorkflowRouteTail([]), { id: null, runId: null });
});

// ---------------------------------------------------------- task navigation

test("a node run with a spawned task yields an action that opens it", () => {
  const calls = [];
  const action = openTaskAction({ node_id: "a", task_id: "t1" }, id => calls.push(id));
  assert.ok(action);
  action();
  assert.deepEqual(calls, ["t1"]);
});

test("a node run with no spawned task yields no action", () => {
  assert.equal(openTaskAction({ node_id: "a", task_id: null }, () => {}), null);
  assert.equal(openTaskAction(undefined, () => {}), null);
});
