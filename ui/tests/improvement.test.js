import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { state } from "../js/core.js";
import { visibleConfigurations, configCard } from "../js/benchmarks.js";
import { findingLabel, renderKnowledge } from "../js/knowledge.js";
import {
  layoutGraph,
  nodeRadius,
  neighborhood,
  connectedNodeIds,
  matchesSearch,
  isNodeDimmed,
  areaColorTokens,
  shouldShowAllLabels,
  unitsPerPixel,
  viewBoxPoint,
  zoomAt,
  panBy,
  nodeTail,
  readNodeTail,
  encodeToolbarFlags,
  decodeToolbarFlags,
} from "../js/knowledge-graph.js";

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

// A gap is drawn as a node too (dashed, in knowledge.js) -- unlike v1's
// `notes`/`gaps` split, a gap target the caller wants on screen is simply
// another entry in the node list `buildNodes` hands `layoutGraph`, so the
// fixture below includes one rather than only referencing it from an edge.
const NODES = [
  { id: "ops/alpha", r: nodeRadius(1) },
  { id: "ops/beta", r: nodeRadius(0) },
  { id: "clients/gamma", r: nodeRadius(0) },
  { id: "ops/missing", r: nodeRadius(0) },
];
const EDGES = [
  ["ops/alpha", "ops/beta"],
  ["ops/alpha", "ops/missing"],
  ["clients/gamma", "ops/alpha"],
];

test("layoutGraph is deterministic: the same input twice gives identical coordinates", () => {
  const a = layoutGraph(NODES, EDGES, { width: 640, height: 420 });
  const b = layoutGraph(NODES, EDGES, { width: 640, height: 420 });
  assert.deepEqual(a, b);
});

test("layoutGraph does not depend on the order nodes and edges arrive in", () => {
  const forward = layoutGraph(NODES, EDGES, { width: 640, height: 420 });
  const shuffled = layoutGraph([...NODES].reverse(), [...EDGES].reverse(), { width: 640, height: 420 });
  assert.deepEqual(forward, shuffled);
});

test("layoutGraph keeps every node's circle inside the viewBox", () => {
  const width = 640;
  const height = 420;
  const pos = layoutGraph(NODES, EDGES, { width, height });
  const radiusOf = new Map(NODES.map((n) => [n.id, n.r]));
  for (const [id, { x, y }] of Object.entries(pos)) {
    const r = radiusOf.get(id);
    assert.ok(x - r >= 0 && x + r <= width, `${id} draws outside the width at x=${x}`);
    assert.ok(y - r >= 0 && y + r <= height, `${id} draws outside the height at y=${y}`);
  }
});

test("layoutGraph on an empty vault returns no nodes rather than throwing", () => {
  assert.deepEqual(layoutGraph([], [], { width: 640, height: 420 }), {});
});

test("layoutGraph never places a node an edge names but the node list does not", () => {
  const edgesWithDangling = [...EDGES, ["ops/alpha", "nowhere"]];
  const pos = layoutGraph(NODES, edgesWithDangling, { width: 640, height: 420 });
  assert.equal(pos.nowhere, undefined);
});

test("nodeRadius grows with backlinks and is capped", () => {
  assert.ok(nodeRadius(0) < nodeRadius(5));
  assert.ok(nodeRadius(5) < nodeRadius(50));
  assert.equal(nodeRadius(50), nodeRadius(1000), "the radius has a ceiling");
});

test("layoutGraph settles 600 nodes in under 2s (bucketed repulsion, not O(n^2))", () => {
  const nodes = [];
  const edges = [];
  for (let i = 0; i < 600; i++) nodes.push({ id: `n${i}`, r: nodeRadius(i % 5) });
  for (let i = 1; i < 600; i++) edges.push([`n${i}`, `n${Math.floor(i / 2)}`]);
  const start = Date.now();
  const pos = layoutGraph(nodes, edges, { width: 900, height: 600 });
  const elapsed = Date.now() - start;
  assert.equal(Object.keys(pos).length, 600);
  assert.ok(elapsed < 2000, `600-node layout took ${elapsed}ms, must be under 2000ms`);
});

// -------------------------------------------------------------- local graph neighbourhood

test("neighborhood at depth 1 is the centre plus its direct neighbours only", () => {
  const edges = [
    ["a", "b"],
    ["b", "c"],
    ["c", "d"],
  ];
  const found = neighborhood("b", edges, 1);
  assert.deepEqual([...found].sort(), ["a", "b", "c"]);
});

test("neighborhood walks edges as undirected, and grows with depth", () => {
  const edges = [
    ["a", "b"],
    ["b", "c"],
    ["c", "d"],
  ];
  assert.deepEqual([...neighborhood("d", edges, 1)].sort(), ["c", "d"]);
  assert.deepEqual([...neighborhood("d", edges, 3)].sort(), ["a", "b", "c", "d"]);
});

test("neighborhood of an id no edge names is just that id", () => {
  assert.deepEqual([...neighborhood("solo", [["a", "b"]], 2)], ["solo"]);
});

// ---------------------------------------------------------------------- search

test("matchesSearch matches a page's label, its id, or a tag's bare name", () => {
  assert.ok(matchesSearch({ id: "partners/acme", label: "Acme Co", kind: "page" }, "acme"));
  assert.ok(matchesSearch({ id: "partners/acme", label: "Acme Co", kind: "page" }, "partners"));
  assert.ok(matchesSearch({ id: "tag:pricing", label: "#pricing", kind: "tag" }, "pricing"));
  assert.ok(!matchesSearch({ id: "partners/acme", label: "Acme Co", kind: "page" }, "gadgets"));
  assert.ok(matchesSearch({ id: "x", label: "X", kind: "page" }, ""), "an empty query matches everything");
});

// ------------------------------------------------------------------- dimming

test("isNodeDimmed: with no query and no focus, nothing is dimmed -- a bare selection never dims the graph", () => {
  const node = { id: "a", label: "A", kind: "page" };
  assert.equal(isNodeDimmed(node, "", null, new Set()), false);
});

test("isNodeDimmed: with a query, dimming follows the search match alone, regardless of focus/neighbours", () => {
  const match = { id: "brochure.pdf", label: "Brochure", kind: "document" };
  const noMatch = { id: "tag:pricing", label: "#pricing", kind: "tag" };
  // Neither is the focus nor a neighbour of it -- under the old rule both
  // would have been dimmed by the focus/neighbour check; a search query
  // must override that entirely.
  assert.equal(isNodeDimmed(match, "brochure", "unrelated", new Set()), false);
  assert.equal(isNodeDimmed(noMatch, "brochure", "unrelated", new Set()), true);
});

test("isNodeDimmed: with no query, dimming follows hover/keyboard focus and its neighbours only", () => {
  const focusNode = { id: "hovered", label: "Hovered", kind: "page" };
  const neighbourNode = { id: "next-door", label: "Next door", kind: "page" };
  const farNode = { id: "far", label: "Far", kind: "page" };
  const neighbours = new Set(["hovered", "next-door"]);
  assert.equal(isNodeDimmed(focusNode, "", "hovered", neighbours), false, "the focus itself is never dimmed");
  assert.equal(isNodeDimmed(neighbourNode, "", "hovered", neighbours), false, "a neighbour of the focus is not dimmed");
  assert.equal(isNodeDimmed(farNode, "", "hovered", neighbours), true, "anything else is dimmed once something is focused");
});

// -------------------------------------------------------------------- area colours

test("areaColorTokens assigns distinct tokens by sorted order, not a name hash", () => {
  const tokens = areaColorTokens(["company", "partners", "operations", "company"]);
  const assigned = new Set(tokens.values());
  assert.equal(assigned.size, 3, "three distinct areas must get three distinct tokens");
  // Sorted: company, operations, partners -- so assignment is order-based
  // and does not depend on the order areas happened to arrive in.
  assert.deepEqual(
    [...tokens.entries()].sort(),
    [
      ["company", "--ly1"],
      ["operations", "--ly2"],
      ["partners", "--ly3"],
    ].sort(),
  );
});

test("areaColorTokens ignores blank areas and cycles past six distinct ones", () => {
  const areas = ["a", "b", "c", "d", "e", "f", "g", "", null, undefined];
  const tokens = areaColorTokens(areas);
  assert.equal(tokens.has(""), false);
  assert.equal(tokens.get("a"), "--ly1");
  assert.equal(tokens.get("g"), "--ly1", "a seventh distinct area cycles back to the first token");
});

// -------------------------------------------------------------------- labels

test("shouldShowAllLabels: a small graph shows every label regardless of zoom", () => {
  assert.equal(shouldShowAllLabels(40, 1), true);
  assert.equal(shouldShowAllLabels(60, 1), true);
});

test("shouldShowAllLabels: a large graph needs the zoom threshold", () => {
  assert.equal(shouldShowAllLabels(600, 1), false);
  assert.equal(shouldShowAllLabels(600, 1.6), true);
});

// --------------------------------------------------------------- pan & zoom

test("unitsPerPixel matches either axis when the rect shares the viewBox's aspect ratio", () => {
  const viewBox = { width: 640, height: 420 };
  const ppu = unitsPerPixel(viewBox, { width: 1280, height: 840 });
  assert.ok(Math.abs(ppu - 0.5) < 1e-9);
});

test("unitsPerPixel uses the constrained (meet) axis when the rect's aspect ratio differs", () => {
  const viewBox = { width: 640, height: 420 };
  // Much wider than the viewBox's own aspect ratio: height is the
  // constrained axis under xMidYMid meet, so its ratio (1) wins, not
  // width's (0.5).
  assert.equal(unitsPerPixel(viewBox, { width: 1280, height: 420 }), 1);
});

test("unitsPerPixel returns 1 for a rect with no measurable size", () => {
  assert.equal(unitsPerPixel({ width: 640, height: 420 }, { width: 0, height: 0 }), 1);
});

test("viewBoxPoint maps the rect's own centre to the viewBox's centre regardless of letterboxing", () => {
  const viewBox = { width: 640, height: 420 };
  // Wider than the viewBox's own aspect ratio -- height is the constrained
  // (meet) axis, width letterboxes, so the content does not start flush
  // with rect.left the way a naive (clientX - rect.left) * ppu assumes.
  const rect = { left: 100, top: 50, width: 1280, height: 420 };
  const centre = viewBoxPoint(viewBox, rect, rect.left + rect.width / 2, rect.top + rect.height / 2);
  assert.ok(Math.abs(centre.x - viewBox.width / 2) < 1e-9, `expected x near 320, got ${centre.x}`);
  assert.ok(Math.abs(centre.y - viewBox.height / 2) < 1e-9, `expected y near 210, got ${centre.y}`);
});

test("viewBoxPoint maps the content's own left edge, not the rect's, to viewBox x=0", () => {
  const viewBox = { width: 640, height: 420 };
  const rect = { left: 100, top: 50, width: 1280, height: 420 };
  // The content is letterboxed by (1280 - 640/1)/2 = 320px on each side at
  // ppu=1 (height-constrained) -- so the content's left edge sits at
  // rect.left + 320, not at rect.left itself.
  const atContentEdge = viewBoxPoint(viewBox, rect, rect.left + 320, rect.top);
  assert.ok(Math.abs(atContentEdge.x) < 1e-9, `expected x near 0 at the content's own edge, got ${atContentEdge.x}`);
  const atRectEdge = viewBoxPoint(viewBox, rect, rect.left, rect.top);
  assert.ok(atRectEdge.x < 0, "the rect's own edge sits outside the content when letterboxed, so x must be negative");
});

test("viewBoxPoint matches the naive (clientX - rect.left) * ppu formula when the rect's aspect matches the viewBox's", () => {
  const viewBox = { width: 640, height: 420 };
  const rect = { left: 10, top: 20, width: 1280, height: 840 }; // exactly 2x, same aspect ratio
  const p = viewBoxPoint(viewBox, rect, 10 + 300, 20 + 150);
  assert.ok(Math.abs(p.x - 150) < 1e-9);
  assert.ok(Math.abs(p.y - 75) < 1e-9);
});

test("zoomAt keeps the anchor's underlying content fixed on screen", () => {
  const view = { scale: 1, tx: 0, ty: 0 };
  const anchor = { x: 100, y: 50 };
  const next = zoomAt(view, 2, anchor);
  assert.equal(next.scale, 2);
  // The local point that was drawn at `anchor` before the zoom must still
  // land on `anchor` after it.
  const localX = (anchor.x - view.tx) / view.scale;
  const localY = (anchor.y - view.ty) / view.scale;
  assert.ok(Math.abs(localX * next.scale + next.tx - anchor.x) < 1e-9);
  assert.ok(Math.abs(localY * next.scale + next.ty - anchor.y) < 1e-9);
});

test("zoomAt does not always zoom around the origin -- a non-origin anchor moves tx/ty away from zero", () => {
  const view = { scale: 1, tx: 0, ty: 0 };
  const next = zoomAt(view, 2, { x: 300, y: 200 });
  assert.notEqual(next.tx, 0);
  assert.notEqual(next.ty, 0);
});

test("zoomAt clamps the resulting scale to min/max", () => {
  const view = { scale: 1, tx: 0, ty: 0 };
  assert.equal(zoomAt(view, 100, { x: 0, y: 0 }, { min: 0.5, max: 4 }).scale, 4);
  assert.equal(zoomAt(view, 0.001, { x: 0, y: 0 }, { min: 0.5, max: 4 }).scale, 0.5);
});

test("panBy is not divided by the view's scale -- the same screen delta pans by the same amount at any zoom", () => {
  // `scale` is passed alongside `tx`/`ty` here (as the real `view` object
  // always carries one) specifically to prove `panBy` never reads it: two
  // otherwise-identical views at very different zoom levels must produce
  // the exact same tx/ty delta for the same screen drag and the same ppu.
  const zoomedIn = panBy({ tx: 10, ty: 20, scale: 4 }, 30, -10, 0.5);
  const zoomedOut = panBy({ tx: 10, ty: 20, scale: 0.5 }, 30, -10, 0.5);
  assert.deepEqual(zoomedIn, zoomedOut, "panBy must not scale its result by view.scale");
  assert.equal(zoomedIn.tx, 10 + 30 * 0.5);
  assert.equal(zoomedIn.ty, 20 + -10 * 0.5);
});

// ------------------------------------------------------------- orphans toggle

test("connectedNodeIds keeps only nodes with a neighbour among the given ids", () => {
  const ids = ["a", "b", "c"]; // c has no edge to another id in this set
  const edges = [
    ["a", "b"],
    ["c", "elsewhere-not-in-set"],
  ];
  assert.deepEqual([...connectedNodeIds(ids, edges)].sort(), ["a", "b"]);
});

test("connectedNodeIds treats a node whose only neighbour was filtered out by another toggle as isolated too", () => {
  // "b" has an edge to "tag:x" in the full data, but the tags toggle has
  // already dropped "tag:x" from the eligible id list -- "b" must count as
  // unconnected in the graph as currently drawn, not as connected because
  // the edge exists somewhere in the payload.
  const ids = ["a", "b"];
  const edges = [["b", "tag:x"]];
  assert.deepEqual([...connectedNodeIds(ids, edges)], []);
});

// -------------------------------------------------------------------- knowledge tail

test("nodeTail/readNodeTail round-trip a plain id, slashes and all", () => {
  const id = "partners/acme";
  assert.deepEqual(nodeTail(id), ["partners", "acme"]);
  assert.equal(readNodeTail(nodeTail(id)), id);
});

test("nodeTail escapes a literal 'task' segment so it never collides with app.js's MODAL marker", () => {
  for (const id of ["task", "operations/task", "task/task", "tag:task"]) {
    const tail = nodeTail(id);
    assert.ok(!tail.includes("task"), `tail for ${JSON.stringify(id)} must not contain a bare "task" segment`);
    assert.equal(readNodeTail(tail), id, "escaping must round-trip cleanly");
  }
});

test("an empty tail names no selection", () => {
  assert.equal(readNodeTail([]), null);
  assert.equal(readNodeTail(undefined), null);
});

test("encodeToolbarFlags/decodeToolbarFlags round-trip the toolbar state", () => {
  const t = { tags: true, documents: false, orphans: true, gaps: false, local: true, depth: 3 };
  const seg = encodeToolbarFlags(t);
  assert.match(seg, /^f-1-0-1-0-1-3$/);
  assert.deepEqual(decodeToolbarFlags(seg), t);
});

test("decodeToolbarFlags returns null for anything that is not a flags segment", () => {
  assert.equal(decodeToolbarFlags("partners"), null);
  assert.equal(decodeToolbarFlags(""), null);
  assert.equal(decodeToolbarFlags(undefined), null);
});

// -------------------------------------------------------------- graph fit-to-box

test("layoutGraph fills most of the viewBox for a small graph rather than clustering in the middle", () => {
  const width = 640;
  const height = 420;
  const pos = layoutGraph(NODES, EDGES, { width, height });
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
  const pos = layoutGraph([{ id: "solo", r: nodeRadius(0) }], [], { width, height });
  const p = pos.solo;
  assert.ok(Math.abs(p.x - width / 2) < 40, `x should be near centre: ${p.x}`);
  assert.ok(Math.abs(p.y - height / 2) < 60, `y should be near centre: ${p.y}`);
});

test("layoutGraph never scales a tiny graph absurdly far apart", () => {
  // Two nodes that link to each other settle close together before any
  // fit-to-box scaling; the scale factor applied on top of that is capped
  // so two nodes cannot end up flung to opposite corners of the viewBox.
  const width = 640;
  const height = 420;
  const nodes = [
    { id: "a", r: nodeRadius(1) },
    { id: "b", r: nodeRadius(1) },
  ];
  const pos = layoutGraph(nodes, [["a", "b"]], { width, height });
  const d = Math.hypot(pos.a.x - pos.b.x, pos.a.y - pos.b.y);
  assert.ok(d < Math.hypot(width, height), `two linked nodes should not be flung apart: d=${d}`);
});

// -------------------------------------------------------- label legibility

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

// -------------------------------------------------------- empty state names the import command

test("renderKnowledge's empty state names the import command, and legacy when it is set", () => {
  const fakeElement = () => ({
    textContent: "",
    hidden: false,
    innerHTML: "",
    style: {},
    classList: { toggle() {}, add() {}, remove() {} },
    querySelectorAll: () => [],
    querySelector: () => null,
    addEventListener: () => {},
  });
  const ids = [
    "knowledge-note", "knowledge-scope-note", "knowledge-error", "knowledge-empty", "knowledge-shell",
    "knowledge-count", "knowledge-graph", "knowledge-inspector", "knowledge-pages", "knowledge-no-pages",
    "knowledge-tags", "knowledge-no-tags", "knowledge-documents", "knowledge-no-documents",
    "knowledge-gaps", "knowledge-no-gaps", "knowledge-findings", "knowledge-no-findings",
    "knowledge-search", "knowledge-toggle-tags", "knowledge-toggle-documents", "knowledge-toggle-orphans",
    "knowledge-toggle-gaps", "knowledge-local", "knowledge-depth", "knowledge-zoom-in", "knowledge-zoom-out",
    "knowledge-zoom-fit", "knowledge-upload", "knowledge-upload-errors", "knowledge-refresh",
  ];
  const elements = Object.fromEntries(ids.map((id) => [id, fakeElement()]));
  globalThis.document = { getElementById: (id) => elements[id] || null };
  globalThis.window = { addEventListener: () => {} };

  try {
    state.knowledge = { root: "/inst/.factory/knowledge", present: false, legacy: null };
    state.knowledgeError = null;
    renderKnowledge();
    assert.match(elements["knowledge-empty"].textContent, /factory knowledge import <dir>/);

    state.knowledge = { root: "/inst/.factory/knowledge", present: false, legacy: "/inst/knowledge/wiki" };
    renderKnowledge();
    assert.match(elements["knowledge-empty"].textContent, /factory knowledge import \/inst\/knowledge\/wiki/);
  } finally {
    delete globalThis.document;
    delete globalThis.window;
    state.knowledge = null;
    state.knowledgeError = null;
  }
});

// -------------------------------------------------------- benchmarks order and disclosure

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
  assert.equal(findingLabel("unsourced"), "Unsourced pages");
  assert.equal(findingLabel("secret_source"), "Sources under data/secrets/");
  assert.equal(findingLabel("missing_source"), "Sources not found");
  assert.equal(findingLabel("incomplete_frontmatter"), "Incomplete frontmatter");
  assert.equal(findingLabel("orphan"), "Orphans — nothing connects here");
  assert.equal(findingLabel("ambiguous_link"), "Ambiguous links");
  assert.equal(findingLabel("truncated"), "Walk truncated");
  assert.equal(findingLabel("some_future_kind"), "some_future_kind");
});

test("a node id containing 'task' survives app.js's own task-modal split unharmed", () => {
  // A faithful copy of `splitTail` in app.js: it cuts a view's tail at the
  // first segment equal to `MODAL` ("task") to tell a task-modal boundary
  // apart from the view's own segments. This proves the escape in
  // `nodeTail` keeps that cut from ever firing on a node id, rather than
  // just asserting the round-trip in isolation.
  const MODAL = "task";
  function splitTail(tail) {
    const cut = tail.indexOf(MODAL);
    return cut < 0 ? [tail, []] : [tail.slice(0, cut), tail.slice(cut + 1)];
  }
  for (const id of ["task", "operations/task", "task/child", "plain/id", "tag:task"]) {
    const tail = nodeTail(id);
    const [view, modal] = splitTail(tail);
    assert.deepEqual(view, tail, `the whole tail for ${JSON.stringify(id)} must stay on the view's side`);
    assert.deepEqual(modal, [], "no task-modal boundary should ever be found inside a node tail");
    assert.equal(readNodeTail(view), id);
  }
});
