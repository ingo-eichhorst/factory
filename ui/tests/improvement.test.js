import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { state } from "../js/core.js";
import { visibleConfigurations, configCard } from "../js/benchmarks.js";
import { layoutGraph, nodeRadius, noteTail, readNoteTail, findingLabel, renderKnowledge } from "../js/knowledge.js";

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const app = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");
const css = readFileSync(new URL("../app.css", import.meta.url), "utf8");
const knowledgeSrc = readFileSync(new URL("../js/knowledge.js", import.meta.url), "utf8");

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

// -------------------------------------------------------------- graph fit-to-box (finding 7)

test("layoutGraph fills most of the viewBox for a small graph rather than clustering in the middle", () => {
  const width = 640;
  const height = 420;
  const pos = layoutGraph(NOTES, GAPS, { width, height });
  const xs = Object.values(pos).map((p) => p.x);
  const ys = Object.values(pos).map((p) => p.y);
  const spanX = Math.max(...xs) - Math.min(...xs);
  const spanY = Math.max(...ys) - Math.min(...ys);
  assert.ok(
    spanX >= width * 0.6 || spanY >= height * 0.6,
    `a small graph should fill most of the box, not cluster in the middle: spanX=${spanX}, spanY=${spanY}`,
  );
});

test("layoutGraph centers a single node rather than pinning it to a corner", () => {
  const width = 640;
  const height = 420;
  const pos = layoutGraph([{ id: "solo", title: "Solo", links: [], gaps: [], backlinks: [] }], [], { width, height });
  const p = pos.solo;
  assert.ok(Math.abs(p.x - width / 2) < 40, `x should be near centre: ${p.x}`);
  assert.ok(Math.abs(p.y - height / 2) < 60, `y should be near centre: ${p.y}`);
});

test("layoutGraph never scales a tiny graph absurdly far apart", () => {
  // Two notes that link to each other settle close together before any
  // fit-to-box scaling; the scale factor applied on top of that is capped
  // so two nodes cannot end up flung to opposite corners of the viewBox.
  const width = 640;
  const height = 420;
  const notes = [
    { id: "a", title: "A", links: ["b"], gaps: [], backlinks: ["b"] },
    { id: "b", title: "B", links: ["a"], gaps: [], backlinks: ["a"] },
  ];
  const pos = layoutGraph(notes, [], { width, height });
  const d = Math.hypot(pos.a.x - pos.b.x, pos.a.y - pos.b.y);
  assert.ok(d < Math.hypot(width, height), `two linked nodes should not be flung apart: d=${d}`);
});

// -------------------------------------------------------- label legibility (finding 8)

test("knowledge.js draws edges, then nodes, then labels -- in that order, so neither paints over a label", () => {
  assert.match(
    knowledgeSrc,
    /<g class="know-edges">\$\{edgeSvg\}<\/g><g class="know-nodes">\$\{nodeSvg\}<\/g><g class="know-labels">\$\{labelSvg\}<\/g>/,
    "labels must be their own layer, drawn after both edges and nodes",
  );
});

test("knowledge graph labels get a halo using the panel background token", () => {
  const block = /\.know-label\s*\{([^}]*)\}/.exec(css);
  assert.ok(block, ".know-label rule must exist in app.css");
  assert.match(block[1], /paint-order:\s*stroke/);
  assert.match(block[1], /stroke:\s*var\(--panel\)/);
  assert.match(block[1], /stroke-width:\s*3px/);
  assert.match(block[1], /stroke-linejoin:\s*round/);
});

// -------------------------------------------------------- empty pages note (finding 9)

test("the pages note is hidden whenever it has no text, including on a failed fetch", () => {
  const fakeElement = () => ({ textContent: "", hidden: false, innerHTML: "", querySelectorAll: () => [] });
  const ids = [
    "knowledge-note",
    "knowledge-scope-note",
    "knowledge-error",
    "knowledge-empty",
    "knowledge-shell",
    "knowledge-count",
    "knowledge-graph",
    "knowledge-inspector",
    "knowledge-gaps",
    "knowledge-no-gaps",
    "knowledge-findings",
    "knowledge-no-findings",
    "knowledge-notes",
    "knowledge-no-notes",
    "knowledge-pages-note",
  ];
  const elements = Object.fromEntries(ids.map((id) => [id, fakeElement()]));
  globalThis.document = { getElementById: (id) => elements[id] || null };

  try {
    state.knowledge = null;
    state.knowledgeError = "could not reach the daemon";
    renderKnowledge();
    assert.equal(elements["knowledge-pages-note"].textContent, "");
    assert.equal(elements["knowledge-pages-note"].hidden, true, "empty text must be hidden on a failed fetch");

    state.knowledge = { root: "/tmp/wiki", present: true, notes: [], gaps: [], pages: ["index.md"], findings: [] };
    state.knowledgeError = null;
    renderKnowledge();
    assert.notEqual(elements["knowledge-pages-note"].textContent, "");
    assert.equal(elements["knowledge-pages-note"].hidden, false, "non-empty text must not be hidden");
  } finally {
    delete globalThis.document;
    state.knowledge = null;
    state.knowledgeError = null;
  }
});

// -------------------------------------------------------- benchmarks order and disclosure (finding 10)

test("the Benchmarks view renders configuration cards before the 'what a score would need' checklist", () => {
  const cards = page.indexOf('id="benchmarks-cards"');
  const needs = page.indexOf('id="benchmarks-needs"');
  assert.ok(cards >= 0 && needs >= 0);
  assert.ok(cards < needs, "cards must come before the checklist, per the issue's own order");
});

test("a configuration card's field disclosure is open by default", () => {
  const html = configCard(CONFIGS[0]);
  assert.match(html, /<details class="bench-details" open>/);
});

// -------------------------------------------------------- readable finding headings (finding 11)

test("findingLabel maps every documented kind to a readable heading, and falls back for an unknown one", () => {
  assert.equal(findingLabel("unsourced"), "Unsourced notes");
  assert.equal(findingLabel("secret_source"), "Sources under data/secrets/");
  assert.equal(findingLabel("missing_source"), "Sources not found");
  assert.equal(findingLabel("incomplete_frontmatter"), "Incomplete frontmatter");
  assert.equal(findingLabel("orphan"), "Orphans — nothing links here");
  assert.equal(findingLabel("ambiguous_link"), "Ambiguous links");
  assert.equal(findingLabel("truncated"), "Walk truncated");
  assert.equal(findingLabel("some_future_kind"), "some_future_kind");
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
