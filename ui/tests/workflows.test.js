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
  reworkPath,
  reworkTargets,
  rootIds,
  topologicalSummary,
  ancestors,
} from "../js/workflow-graph.js";
import {
  addEdge,
  applyWorkflowEvent,
  duplicateNode,
  editorButtons,
  inputProblems,
  isInputName,
  newTaskNode,
  nodeInputUsage,
  openTaskAction,
  placeholders,
  readWorkflowRouteTail,
  reworkBadge,
  reworkProblems,
  reworkRequestText,
  reworkSentence,
  roundLabel,
  runInputs,
  saveDraft,
  setInputField,
  supersededTasks,
  validate,
  workflowRouteTail,
} from "../js/workflow-model.js";

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const app = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");
const css = readFileSync(new URL("../app.css", import.meta.url), "utf8");
const view = readFileSync(new URL("../js/workflows.js", import.meta.url), "utf8");

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
  assert.equal(NODE_STATUSES.length, 11, "11 WorkflowNodeStatus variants (skipped_by_route: #149)");
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

test("running and verifying are blue while done and completed edges stay green", () => {
  const rule = selector => {
    const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    return new RegExp(`${escaped}\\s*\\{([^}]*)\\}`).exec(css)?.[1] || "";
  };
  const runningNode = rule(".workflow-node.wf-s-running");
  const verifyingNode = rule(".workflow-node.wf-s-verifying");
  const runningBadge = rule(".wf-badge.wf-s-running");
  const verifyingBadge = rule(".wf-badge.wf-s-verifying");
  const doneNode = rule(".workflow-node.wf-s-done");
  const doneBadge = rule(".wf-badge.wf-s-done");
  const doneEdge = rule(".workflow-edge.wf-edge-done");

  assert.match(runningNode, /border-left:\s*4px solid var\(--signal\)/);
  assert.match(runningNode, /background:\s*var\(--signal-wash\)/);
  assert.match(runningNode, /box-shadow:\s*0 0 0 1px var\(--signal\)/);
  assert.match(verifyingNode, /border-left:\s*3px dotted var\(--signal\)/);
  assert.match(verifyingNode, /background:\s*var\(--signal-wash\)/);
  for (const badge of [runningBadge, verifyingBadge]) {
    assert.match(badge, /color:\s*var\(--signal-ink\)/);
    assert.match(badge, /background:\s*var\(--signal-wash\)/);
  }
  assert.match(doneNode, /var\(--run\)/);
  assert.match(doneBadge, /var\(--run\)/);
  assert.match(doneEdge, /var\(--run\)/);
});

test("every Workflows run-status surface uses the workflow badge classes", () => {
  assert.match(view, /class="wf-badge \$\{nodeStatusClass\(run\.status\)\}"/);
  assert.equal(
    view.match(/class="wf-badge \$\{nodeStatusClass\(currentRun\.status\)\}"/g)?.length,
    2,
    "the toolbar and inspector statuses must both share the workflow badge palette",
  );
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

// ------------------------------------------------- #143: inputs and rework

/// `workflows/github-issue.yaml`'s shape, reduced to what these rules read:
/// triage -> implement -> review -> ready, review sending work back to
/// implement at most five times, and a gate hung off implement.
function githubIssue() {
  const n = (id, title, extra = {}) => ({ id, position: { x: 0, y: 0 }, kind: "task", task: { title, instructions: "", labels: {} }, ...extra });
  return {
    name: "github-issue", scope: "factory",
    inputs: [{ name: "issue", description: "GitHub issue number" }],
    nodes: [
      n("triage", "Triage #{{issue}}"),
      n("implement", "Implement #{{issue}}"),
      n("review", "Review #{{issue}}", { exits: [{ to: "implement", agent: "concrete findings the implementer can fix alone", max_rounds: 5 }] }),
      n("ready", "Mark PR for #{{issue}} ready"),
      n("tests", "tests", { kind: "gate", gate: { step: "tests", command: "cargo test" } }),
    ],
    edges: [
      { id: "ti", from: "triage", to: "implement" },
      { id: "ir", from: "implement", to: "review" },
      { id: "rr", from: "review", to: "ready" },
      { id: "it", from: "implement", to: "tests" },
      { id: "tr", from: "tests", to: "review" },
    ],
  };
}

test("#143 the github-issue shape validates clean", () => {
  assert.deepEqual(validate(githubIssue()), []);
});

test("#143 placeholders: the same cases as workflow.rs -- spaces allowed, non-names left alone", () => {
  assert.deepEqual(placeholders("Triage #{{issue}} and {{ issue }} for {{repo-name}}"), ["issue", "issue", "repo-name"]);
  assert.deepEqual(placeholders("docker ps --format {{.Names}} {{}} {{ 9lives }}"), []);
  assert.deepEqual(placeholders("{{{a}}"), [], "the scan starts at the first `{{`, so `{a` is no name");
  assert.deepEqual(placeholders("{{a}} then {{unclosed"), ["a"]);
  assert.deepEqual(placeholders(undefined), []);
});

test("#143 isInputName: a letter or `_` first, then letters, digits, `_` or `-`", () => {
  for (const good of ["issue", "_x", "repo-name", "a1"]) assert.equal(isInputName(good), true, good);
  for (const bad of ["", "1a", "-a", "a b", ".Names", "a.b"]) assert.equal(isInputName(bad), false, bad);
});

test("#143 inputs: a bad name and a duplicate are refused", () => {
  const wf = githubIssue();
  wf.inputs = [{ name: "issue" }, { name: "issue" }, { name: "9lives" }];
  const messages = inputProblems(wf).map(e => e.message);
  assert.ok(messages.some(m => /"issue" is declared twice/.test(m)), messages.join("\n"));
  assert.ok(messages.some(m => /"9lives" is not a name/.test(m)), messages.join("\n"));
});

test("#143 inputs: an undeclared {{x}} in a title, instructions or label value is refused, keyed to its node", () => {
  for (const place of ["title", "instructions", "label"]) {
    const wf = githubIssue();
    const triage = wf.nodes[0];
    if (place === "title") triage.task.title = "Triage {{isue}}";
    if (place === "instructions") triage.task.instructions = "Look at {{ isue }}";
    if (place === "label") triage.task.labels = { issue: "{{isue}}" };
    const errors = validate(wf);
    const error = errors.find(e => /\{\{isue\}\}/.test(e.message));
    assert.ok(error, `${place}: ${JSON.stringify(errors)}`);
    assert.equal(error.nodeId, "triage");
    assert.match(error.message, /"Triage/);
  }
});

test("#143 inputs: with none declared, braces are left alone, as on the server", () => {
  const wf = githubIssue();
  wf.inputs = [];
  wf.nodes[0].task.instructions = "{{isue}} and {{.Names}}";
  assert.deepEqual(validate(wf), []);
});

test("#143 inputs: a gate's text is never checked for placeholders", () => {
  const wf = githubIssue();
  wf.nodes[4].task.title = "tests for {{nothing}}";
  assert.deepEqual(inputProblems(wf), []);
});

test("#143 the inspector's uses line lists each input once and marks the undeclared", () => {
  const n = { id: "a", kind: "task", task: { title: "{{issue}}", instructions: "{{ issue }} {{repo}}", labels: { x: "{{branch}}" } } };
  assert.deepEqual(nodeInputUsage(n, [{ name: "issue" }]), { used: ["issue", "repo", "branch"], undeclared: ["repo", "branch"] });
  // No input declared: the server leaves braces as literal text, so none
  // is marked -- a mark would name an error Save never raises.
  assert.deepEqual(nodeInputUsage(n, undefined), { used: ["issue", "repo", "branch"], undeclared: [] });
  assert.deepEqual(nodeInputUsage(n, []).undeclared, []);
});

test("#145 an input edit lands in the workflow passed in, not one captured before a reload", () => {
  const before = { inputs: [{ name: "issue", description: "old" }] };
  const reloaded = structuredClone(before); // what loadWorkflows() puts in `current`
  assert.equal(setInputField(reloaded, 0, "description", "new"), true);
  assert.equal(reloaded.inputs[0].description, "new");
  assert.equal(before.inputs[0].description, "old");
  assert.deepEqual(saveDraft({ ...githubIssue(), inputs: reloaded.inputs }).inputs, [{ name: "issue", description: "new" }]);
});

test("#145 setInputField refuses a row or field that is not there", () => {
  const wf = { inputs: [{ name: "issue", description: "" }] };
  assert.equal(setInputField(wf, 1, "name", "x"), false);
  assert.equal(setInputField(wf, 0, "nodes", "x"), false);
  assert.equal(setInputField({}, 0, "name", "x"), false);
  assert.deepEqual(wf.inputs, [{ name: "issue", description: "" }]);
});

test("#143 ancestors: everything a node can be reached from, never itself", () => {
  const { edges } = githubIssue();
  assert.deepEqual([...ancestors(edges, "review")].sort(), ["implement", "tests", "triage"]);
  assert.deepEqual([...ancestors(edges, "triage")], []);
});

test("#143 rework targets on github-issue: ancestor task nodes only, in definition order, never a gate", () => {
  const { nodes, edges } = githubIssue();
  const ids = id => reworkTargets(nodes, edges, id).map(n => n.id);
  assert.deepEqual(ids("review"), ["triage", "implement"]);
  assert.deepEqual(ids("implement"), ["triage"]);
  assert.deepEqual(ids("triage"), []);
});

test("#143 rework targets: a node with no kind is a task, as the server's default has it", () => {
  const nodes = [{ id: "a", task: { title: "a" } }, { id: "b", task: { title: "b" } }];
  assert.deepEqual(reworkTargets(nodes, [{ id: "ab", from: "a", to: "b" }], "b").map(n => n.id), ["a"]);
});

test("#143 rework: a deleted target is refused, naming the node and keyed to it", () => {
  const wf = githubIssue();
  const cut = removeNode(wf.nodes, wf.edges, "implement");
  const errors = reworkProblems({ ...wf, ...cut });
  assert.equal(errors.length, 1, JSON.stringify(errors));
  assert.equal(errors[0].nodeId, "review");
  assert.match(errors[0].message, /"Review #\{\{issue\}\}" exit 1 targets a node that no longer exists/);
  assert.ok(validate({ ...wf, ...cut }).some(e => e.nodeId === "review"), "validate refuses it too");
});

test("#143 rework: a target that no longer comes before the node is refused", () => {
  const wf = githubIssue();
  // Both paths from implement to review go: the direct link and the gate's.
  wf.edges = removeEdge(removeEdge(wf.edges, "ir"), "tr");
  const errors = reworkProblems(wf);
  assert.equal(errors.length, 1, JSON.stringify(errors));
  assert.equal(errors[0].nodeId, "review");
  assert.match(errors[0].message, /points forward to "Implement #\{\{issue\}\}" without an explicit link/);
});

test("#143 rework: one remaining path is enough to keep the target an ancestor", () => {
  const wf = githubIssue();
  wf.edges = removeEdge(wf.edges, "ir"); // implement -> tests -> review still stands
  assert.deepEqual(reworkProblems(wf), []);
});

test("#143 rework: a gate as target, a gate sending work back, and zero rounds are refused", () => {
  const toGate = githubIssue();
  toGate.nodes[2].exits = [{ to: "tests", agent: "retry", max_rounds: 1 }];
  assert.match(reworkProblems(toGate)[0].message, /back to the gate "tests"; name a task node/);

  const fromGate = githubIssue();
  fromGate.nodes[4].exits = [{ to: "implement", agent: "retry", max_rounds: 1 }];
  const gateError = reworkProblems(fromGate).find(e => e.nodeId === "tests");
  assert.match(gateError.message, /is a gate and cannot declare exits/);

  for (const rounds of [0, null, undefined, 1.5]) {
    const zero = githubIssue();
    zero.nodes[2].exits[0].max_rounds = rounds;
    const errors = reworkProblems(zero);
    assert.equal(errors.length, 1, `${rounds}: ${JSON.stringify(errors)}`);
    assert.match(errors[0].message, /at least one round/);
    assert.equal(errors[0].nodeId, "review");
  }
});

test("#149 exits require exactly one non-empty condition", () => {
  for (const exit of [
    { to: "implement" },
    { to: "implement", check: "true", agent: "choose it" },
  ]) {
    const wf = githubIssue();
    wf.nodes[2].exits = [exit];
    assert.match(reworkProblems(wf)[0].message, /exactly one of check or agent/);
  }
  for (const exit of [
    { to: "implement", check: "  " },
    { to: "implement", agent: "" },
  ]) {
    const wf = githubIssue();
    wf.nodes[2].exits = [exit];
    assert.match(reworkProblems(wf)[0].message, /empty condition/);
  }
});

test("#143 rework: how the canvas and the summary word it", () => {
  const { nodes } = githubIssue();
  assert.equal(reworkSentence(nodes, nodes[2]), "exit 1 → Implement #{{issue}} (agent concrete findings the implementer can fix alone, at most 5×)");
  assert.equal(reworkBadge(nodes, nodes[2]), "1. agent → Implement #{{issue}} ×5");
  assert.equal(reworkSentence(nodes, nodes[0]), "");
  assert.equal(reworkBadge(nodes, nodes[0]), "");
});

test("#143 the rework curve runs from the bottom of one card to the bottom of the other, below both", () => {
  const from = { position: { x: 640, y: 0 } }, to = { position: { x: 320, y: 0 } };
  const d = reworkPath(from, to, 100, 84);
  const [x1, y1, , depth1, , depth2, x2, y2] = d.match(/-?[\d.]+/g).map(Number);
  assert.equal(y1, 100, "starts on the sending card's rendered bottom");
  assert.equal(y2, 84, "ends on the target card's rendered bottom");
  assert.ok(x1 > x2, "runs back, right to left");
  assert.ok(depth1 > 100 && depth2 > 100, "dips below both cards");
});

test("#143 duplicating a node leaves its rework behind: the copy has nothing before it", () => {
  const { nodes } = githubIssue();
  const copy = duplicateNode(nodes, "review");
  assert.equal(copy.exits, undefined);
  assert.equal(copy.task.title, "Review #{{issue}} copy");
});

test("#149 the save draft keeps inputs, each node's ordered exits and the category exactly as they are", () => {
  const wf = { ...githubIssue(), description: "", category: "delivery", id: "wf-1", revision: 3 };
  const draft = JSON.parse(JSON.stringify(saveDraft(wf)));
  assert.deepEqual(draft.inputs, wf.inputs);
  assert.deepEqual(draft.nodes.find(n => n.id === "review").exits, [{ to: "implement", agent: "concrete findings the implementer can fix alone", max_rounds: 5 }]);
  assert.equal(draft.nodes.find(n => n.id === "triage").exits, undefined, "no exits are sent as none, not null");
  assert.equal(draft.category, "delivery");
  assert.deepEqual(draft.edges, wf.edges);
  assert.equal(draft.id, undefined, "the id and revision are the URL's and the server's, not the draft's");
});

test("#143 a definition the server sent without inputs saves an empty list", () => {
  const wf = githubIssue();
  delete wf.inputs;
  assert.deepEqual(saveDraft(wf).inputs, []);
});

test("#149 the page has the inputs list and ordered-exit editor", () => {
  assert.match(page, /id="workflow-inputs"/);
  assert.match(page, /id="workflow-input-add"/);
  assert.match(page, /id="workflow-node-exits-list"/);
  assert.match(page, /id="workflow-node-exit-add"/);
});

// ------------------------------------------------ #143: rework in Run mode

/// A run of review -> implement after one send-back, in the shape
/// `workflow.rs`'s `send_back` test leaves it: implement and review both on
/// round 1, implement's first task superseded and told who sent it back.
function reworkedRun() {
  const definition = githubIssue();
  definition.nodes[1].task.title = "Implement #143";
  definition.nodes[2].task.title = "Review #143";
  return {
    id: "run-1", workflow_id: "wf-1", revision: 3, status: "running", definition,
    inputs: { issue: "143" },
    nodes: [
      { node_id: "triage", status: "done", task_id: "t1" },
      { node_id: "implement", status: "running", task_id: "i2", round: 1, superseded_task_ids: ["i1"],
        rework_request: { from_node: "review", from_task: "r1", round: 1, max_rounds: 5 } },
      { node_id: "review", status: "pending", round: 1, superseded_task_ids: ["r1"] },
      { node_id: "ready", status: "pending" },
    ],
  };
}

test("#143 a node run after a send-back reads 'rework 1'; a first pass reads nothing", () => {
  const run = reworkedRun();
  assert.equal(roundLabel(run.nodes[1]), "rework 1");
  assert.equal(roundLabel(run.nodes[2]), "rework 1");
  assert.equal(roundLabel(run.nodes[0]), "", "round 0 is omitted on the wire");
  assert.equal(roundLabel(undefined), "", "Design mode has no node run at all");
});

test("#143 the superseded tasks are the earlier rounds', oldest first", () => {
  const run = reworkedRun();
  assert.deepEqual(supersededTasks(run.nodes[1]), ["i1"]);
  assert.deepEqual(supersededTasks(run.nodes[3]), []);
  assert.deepEqual(supersededTasks(null), []);
});

test("#143 the target node says who sent the work back, by the run's own title, and which round", () => {
  const run = reworkedRun();
  assert.equal(reworkRequestText(run.nodes[1], run.definition.nodes), "sent back by Review #143, round 1 of 5");
  assert.equal(reworkRequestText(run.nodes[2], run.definition.nodes), "", "the sender carries no request");
});

test("#143 the run's inputs are listed; a run the server sent without any lists none", () => {
  assert.deepEqual(runInputs(reworkedRun()), [["issue", "143"]]);
  const bare = reworkedRun();
  delete bare.inputs;
  assert.deepEqual(runInputs(bare), []);
});
