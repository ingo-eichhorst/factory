import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { topologicalSummary } from "../js/workflow-graph.js";

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const app = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");
const source = readFileSync(new URL("../js/workflows.js", import.meta.url), "utf8");

test("Workflows is a Process peer of Tasks with an accessible canvas and summary", () => {
  assert.match(page, /id="tab-tasks"[^>]*>Tasks<\/button>\s*<button id="tab-workflows"[^>]*>Workflows<\/button>/);
  assert.match(page, /id="workflow-canvas"[^>]*tabindex="0"/);
  assert.match(page, /id="workflow-summary"/);
  assert.match(app, /proc: \["tasks", "workflows"\]/);
});

test("the textual order represents fan-out and fan-in", () => {
  const workflow = {
    nodes: ["a", "b", "c", "d"].map(id => ({ id, task: { title: id } })),
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

test("cycles are actionable and task nodes link to ordinary task details", () => {
  const summary = topologicalSummary({
    nodes: [{ id: "a" }, { id: "b" }],
    edges: [{ from: "a", to: "b" }, { from: "b", to: "a" }],
  });
  assert.match(summary.error, /Cycle involving a, b/);
  assert.match(source, /openTask\(task\.dataset\.task\)/);
  assert.match(source, /workflow_run_updated/);
});
