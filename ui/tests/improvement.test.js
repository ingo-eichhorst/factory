import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { visibleConfigurations } from "../js/benchmarks.js";
import { layoutGraph, nodeRadius, noteTail, readNoteTail } from "../js/knowledge.js";

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const app = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");

// -------------------------------------------------------------------- L5 level

test("L5 Improvement is live, with a Benchmarks tab and a Knowledge tab after Secrets", () => {
  assert.doesNotMatch(page, /id="lv-imp"[^>]*disabled/);
  assert.match(page, /id="tab-benchmarks"[^>]*>Benchmarks<\/button>/);
  assert.match(page, /id="tab-knowledge"[^>]*>Knowledge<\/button>/);
  assert.match(page, /id="view-benchmarks"/);
  assert.match(page, /id="view-knowledge"/);

  // The tripwire: new tabs must come after `#tab-secrets`, and stay adjacent
  // to each other in the order `LEVEL_VIEWS.imp` names them.
  const secrets = page.indexOf('id="tab-secrets"');
  const benchmarks = page.indexOf('id="tab-benchmarks"');
  const knowledge = page.indexOf('id="tab-knowledge"');
  assert.ok(secrets < benchmarks, "Benchmarks tab must follow Secrets");
  assert.ok(benchmarks < knowledge, "Knowledge tab must follow Benchmarks");
});

test("LEVEL_VIEWS.imp names exactly benchmarks and knowledge, invented spelling and all", () => {
  assert.match(app, /imp: \["benchmarks", "knowledge"\]/);
});

test("both new views have their own error element, hidden by default", () => {
  assert.match(page, /id="benchmarks-error"[^>]*hidden/);
  assert.match(page, /id="knowledge-error"[^>]*hidden/);
});

// -------------------------------------------------------------------- benchmarks

const CONFIGS = [
  {
    harness: "acme-harness",
    model: "opus-quarry",
    model_source: "args",
    flags: ["--permission-mode plan", "--yolo"],
    sandbox: "none",
    missing: ["harness version", "tool surface", "context policy", "retry budget"],
    pinned: false,
    agents: [
      { scope: "projects/quarry", agent: "builder", lifetime: "temporary", declared: true },
      { scope: "projects/kiln", agent: "reviewer", lifetime: "permanent", declared: true },
    ],
  },
  {
    harness: "foreman-shell",
    model: null,
    model_source: null,
    flags: [],
    sandbox: "srt",
    missing: ["harness version", "model", "tool surface", "context policy", "retry budget"],
    pinned: false,
    agents: [{ scope: "projects/kiln", agent: "foreman", lifetime: "permanent", declared: false }],
  },
];

test("visibleConfigurations keeps a configuration with at least one agent in scope, narrowed to it", () => {
  const inQuarry = (scope) => scope === "projects/quarry";
  const kept = visibleConfigurations(CONFIGS, inQuarry);
  assert.equal(kept.length, 1);
  assert.equal(kept[0].harness, "acme-harness");
  assert.deepEqual(kept[0].agents.map((a) => a.agent), ["builder"]);
});

test("visibleConfigurations drops a configuration with no agent in scope", () => {
  const inNowhere = () => false;
  assert.deepEqual(visibleConfigurations(CONFIGS, inNowhere), []);
});

test("visibleConfigurations never mutates the cached answer -- widening the scope again finds every agent", () => {
  const original = JSON.parse(JSON.stringify(CONFIGS));
  visibleConfigurations(CONFIGS, (scope) => scope === "projects/quarry");
  assert.deepEqual(CONFIGS, original, "the source configurations must be untouched");

  const wide = visibleConfigurations(CONFIGS, () => true);
  assert.equal(wide[0].agents.length, 2, "a configuration's full agent list must still be there");
});

// -------------------------------------------------------------------- knowledge graph

const NOTES = [
  { id: "ops/alpha", title: "Alpha", links: ["ops/beta"], gaps: ["ops/missing"], backlinks: ["ops/beta"] },
  { id: "ops/beta", title: "Beta", links: [], gaps: [], backlinks: [] },
  { id: "clients/gamma", title: "Gamma", links: ["ops/alpha"], gaps: [], backlinks: [] },
];
const GAPS = [{ target: "ops/missing", from: ["ops/alpha"] }];

test("layoutGraph is deterministic: the same input twice gives identical coordinates", () => {
  const a = layoutGraph(NOTES, GAPS, { width: 640, height: 420 });
  const b = layoutGraph(NOTES, GAPS, { width: 640, height: 420 });
  assert.deepEqual(a, b);
});

test("layoutGraph does not depend on the order notes and gaps arrive in", () => {
  const forward = layoutGraph(NOTES, GAPS, { width: 640, height: 420 });
  const shuffled = layoutGraph([...NOTES].reverse(), [...GAPS], { width: 640, height: 420 });
  assert.deepEqual(forward, shuffled);
});

test("layoutGraph keeps every node's circle inside the viewBox", () => {
  const width = 640;
  const height = 420;
  const pos = layoutGraph(NOTES, GAPS, { width, height });
  const backlinksOf = new Map(NOTES.map((n) => [n.id, (n.backlinks || []).length]));
  for (const [id, { x, y }] of Object.entries(pos)) {
    const r = nodeRadius(backlinksOf.get(id) || 0);
    assert.ok(x - r >= 0 && x + r <= width, `${id} draws outside the width at x=${x}`);
    assert.ok(y - r >= 0 && y + r <= height, `${id} draws outside the height at y=${y}`);
  }
});

test("layoutGraph on an empty wiki returns no nodes rather than throwing", () => {
  assert.deepEqual(layoutGraph([], [], { width: 640, height: 420 }), {});
});

test("nodeRadius grows with backlinks and is capped", () => {
  assert.ok(nodeRadius(0) < nodeRadius(5));
  assert.ok(nodeRadius(5) < nodeRadius(50));
  assert.equal(nodeRadius(50), nodeRadius(1000), "the radius has a ceiling");
});

// -------------------------------------------------------------------- knowledge tail

test("noteTail/readNoteTail round-trip a plain id, slashes and all", () => {
  const id = "partners/acme";
  assert.deepEqual(noteTail(id), ["partners", "acme"]);
  assert.equal(readNoteTail(noteTail(id)), id);
});

test("noteTail escapes a literal 'task' segment so it never collides with app.js's MODAL marker", () => {
  for (const id of ["task", "operations/task", "task/task"]) {
    const tail = noteTail(id);
    assert.ok(!tail.includes("task"), `tail for ${JSON.stringify(id)} must not contain a bare "task" segment`);
    assert.equal(readNoteTail(tail), id, "escaping must round-trip cleanly");
  }
});

test("an empty tail names no selection", () => {
  assert.equal(readNoteTail([]), null);
  assert.equal(readNoteTail(undefined), null);
});

test("a note id containing 'task' survives app.js's own task-modal split unharmed", () => {
  // A faithful copy of `splitTail` in app.js: it cuts a view's tail at the
  // first segment equal to `MODAL` ("task") to tell a task-modal boundary
  // apart from the view's own segments. This proves the escape in
  // `noteTail` keeps that cut from ever firing on a note id, rather than
  // just asserting the round-trip in isolation.
  const MODAL = "task";
  function splitTail(tail) {
    const cut = tail.indexOf(MODAL);
    return cut < 0 ? [tail, []] : [tail.slice(0, cut), tail.slice(cut + 1)];
  }
  for (const id of ["task", "operations/task", "task/child", "plain/id"]) {
    const tail = noteTail(id);
    const [view, modal] = splitTail(tail);
    assert.deepEqual(view, tail, `the whole tail for ${JSON.stringify(id)} must stay on the view's side`);
    assert.deepEqual(modal, [], "no task-modal boundary should ever be found inside a note tail");
    assert.equal(readNoteTail(view), id);
  }
});
