import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { state } from "../js/core.js";
import {
  TILE_KINDS,
  SIZES,
  GRID_COLUMNS,
  VIEW_IDS,
  EPIC_VIEW_IDS,
  RENDERABLE_VIEW_IDS,
  DEFAULT_DASHBOARD,
  tileKind,
  tileSpan,
  validateTile,
  validateDashboard,
  packRows,
  resolveDashboard,
  isTileRenderable,
} from "../js/dashboard-model.js";

// --------------------------------------------------------- the vocabulary

test("tile kinds are exactly metric and view", () => {
  assert.deepEqual(TILE_KINDS, ["metric", "view"]);
});

test("sizes span a 12-column row, and l + m fill one exactly", () => {
  assert.equal(GRID_COLUMNS, 12);
  assert.deepEqual(Object.keys(SIZES).sort(), ["l", "m", "s", "xl"]);
  assert.equal(SIZES.xl.cols, 12, "xl is a full row");
  // The one constraint today's page fixes: the Throughput/On-the-line row
  // is a 2fr/1fr CSS grid (`.drow` in app.css) -- an 8:4 split matches that
  // 2:1 ratio and leaves l + m === 12, which is what lets `packRows` put
  // them in one row without either card being told to.
  assert.equal(SIZES.l.cols, 8);
  assert.equal(SIZES.m.cols, 4);
  assert.equal(SIZES.l.cols + SIZES.m.cols, GRID_COLUMNS);
});

test("the epic's view vocabulary is #150's ten ids; `kpis` is a phase-2 stopgap, not one of them", () => {
  assert.deepEqual(EPIC_VIEW_IDS, [
    "throughput",
    "on_the_line",
    "production_year",
    "by_scope",
    "agent_hours_by_scope",
    "agent_hours_by_agent",
    "occupancy_strip",
    "inbox",
    "compliance",
    "cost",
  ]);
  assert.ok(!EPIC_VIEW_IDS.includes("kpis"));
  assert.ok(VIEW_IDS.includes("kpis"));
  for (const id of EPIC_VIEW_IDS) assert.ok(VIEW_IDS.includes(id), `${id} is a valid tile id`);
});

test("phase 2 renders exactly kpis, throughput, on_the_line, production_year, by_scope -- nothing new", () => {
  assert.deepEqual(RENDERABLE_VIEW_IDS, ["kpis", "throughput", "on_the_line", "production_year", "by_scope"]);
  for (const id of RENDERABLE_VIEW_IDS) assert.ok(VIEW_IDS.includes(id));
});

// --------------------------------------------------------- tileKind/tileSpan

test("tileKind reads the one key a well-shaped tile has", () => {
  assert.equal(tileKind({ view: "throughput", size: "l" }), "view");
  assert.equal(tileKind({ metric: "throughput_week", size: "s" }), "metric");
  assert.equal(tileKind({ view: "throughput", metric: "throughput_week", size: "l" }), null, "both is not a tile");
  assert.equal(tileKind({ size: "l" }), null, "neither is not a tile");
  assert.equal(tileKind(null), null);
  assert.equal(tileKind("throughput"), null);
});

test("tileSpan reads the columns a tile's size spans, or null when the size is unknown", () => {
  assert.equal(tileSpan({ view: "throughput", size: "l" }), 8);
  assert.equal(tileSpan({ view: "throughput", size: "xxl" }), null);
});

// ----------------------------------------------------------------- validate

test("every DEFAULT_DASHBOARD tile is valid, and so is the whole list", () => {
  for (const tile of DEFAULT_DASHBOARD) {
    assert.deepEqual(validateTile(tile), [], `${JSON.stringify(tile)} should be valid`);
  }
  assert.deepEqual(validateDashboard(DEFAULT_DASHBOARD), []);
});

test("validateTile rejects an unknown view id", () => {
  const errors = validateTile({ view: "gross_margin", size: "s" });
  assert.equal(errors.length, 1);
  assert.match(errors[0], /unknown view tile/);
});

test("validateTile rejects an unknown size", () => {
  const errors = validateTile({ view: "throughput", size: "huge" });
  assert.equal(errors.length, 1);
  assert.match(errors[0], /unknown size/);
});

test("validateTile rejects a tile naming both view and metric, or neither", () => {
  assert.match(validateTile({ view: "throughput", metric: "throughput_week", size: "l" })[0], /exactly one/);
  assert.match(validateTile({ size: "l" })[0], /exactly one/);
});

test("validateTile accepts a structurally sound metric tile -- the registry check is server-side, not here", () => {
  assert.deepEqual(validateTile({ metric: "throughput_week", size: "s" }), []);
  assert.match(validateTile({ metric: "", size: "s" })[0], /non-empty metric id/);
  assert.match(validateTile({ metric: "   ", size: "s" })[0], /non-empty metric id/);
});

test("validateTile rejects non-objects, and validateDashboard rejects an empty or non-array list", () => {
  assert.match(validateTile(null)[0], /must be an object/);
  assert.match(validateTile("throughput")[0], /must be an object/);
  assert.deepEqual(validateDashboard([]), ["a dashboard needs at least one tile"]);
  assert.deepEqual(validateDashboard(null), ["a dashboard needs at least one tile"]);
});

test("validateDashboard prefixes each tile's errors with its position", () => {
  const errors = validateDashboard([{ view: "throughput", size: "l" }, { view: "nope", size: "l" }]);
  assert.deepEqual(errors, ['tile 1: unknown view tile "nope"']);
});

// --------------------------------------------------------------- packRows

test("packRows greedily fills a 12-column row and starts a new one when a tile would overflow it", () => {
  const s = { view: "throughput", size: "s" }; // 2 cols
  const m = { view: "throughput", size: "m" }; // 4 cols
  const l = { view: "throughput", size: "l" }; // 8 cols
  const xl = { view: "throughput", size: "xl" }; // 12 cols

  assert.deepEqual(packRows([s, s, s, s, s, s]), [[s, s, s, s, s, s]], "6 x 2 fills one row exactly");
  assert.deepEqual(packRows([s, s, s, s, s, s, s]), [[s, s, s, s, s, s], [s]], "a 7th overflows into a new row");
  assert.deepEqual(packRows([l, m]), [[l, m]], "l + m fills a row exactly, together");
  // packRows itself doesn't care which order fills a row -- only the
  // running total -- but dashboard.js's `.drow` is a fixed 2fr/1fr grid,
  // not a general span -> grid-column mapping, so it only ever renders an
  // `[l, m]` row correctly. DEFAULT_DASHBOARD never produces `[m, l]`; this
  // case documents the packer's own indifference, not a claim that
  // dashboard.js would render it the same way.
  assert.deepEqual(packRows([m, l]), [[m, l]], "the packer doesn't care about order, only the running total");
  assert.deepEqual(packRows([xl, xl]), [[xl], [xl]], "an xl tile is always alone");
  assert.deepEqual(packRows([]), []);
});

test("packRows on DEFAULT_DASHBOARD reproduces today's four rows: kpis alone, throughput+on_the_line together, then production_year and by_scope each alone", () => {
  const rows = packRows(DEFAULT_DASHBOARD);
  assert.deepEqual(
    rows.map((row) => row.map((t) => t.view)),
    [["kpis"], ["throughput", "on_the_line"], ["production_year"], ["by_scope"]]
  );
});

// ------------------------------------------------------------ DEFAULT_DASHBOARD

test("DEFAULT_DASHBOARD reproduces today's page: same cards, same order, same sizes", () => {
  assert.deepEqual(
    DEFAULT_DASHBOARD.map((t) => [t.view, t.size]),
    [
      ["kpis", "xl"],
      ["throughput", "l"],
      ["on_the_line", "m"],
      ["production_year", "xl"],
      ["by_scope", "xl"],
    ]
  );
});

test("DEFAULT_DASHBOARD is frozen two levels deep", () => {
  assert.ok(Object.isFrozen(DEFAULT_DASHBOARD));
  for (const tile of DEFAULT_DASHBOARD) assert.ok(Object.isFrozen(tile));
});

// --------------------------------------------------- phase 4 (#159) reads

test("resolveDashboard uses the fetched tiles when they are a non-empty array, else the default", () => {
  const fetched = [{ view: "kpis", size: "s" }];
  assert.equal(resolveDashboard(fetched), fetched);
  assert.equal(resolveDashboard(null), DEFAULT_DASHBOARD, "the server's own built-in-default answer");
  assert.equal(resolveDashboard(undefined), DEFAULT_DASHBOARD, "not fetched yet");
  assert.equal(resolveDashboard([]), DEFAULT_DASHBOARD, "the server never sends this -- an empty list is a config error");
  assert.equal(resolveDashboard("nope"), DEFAULT_DASHBOARD, "anything that is not an array reads as no override");
});

test("isTileRenderable is true only for a view id this page already draws a card for", () => {
  for (const id of RENDERABLE_VIEW_IDS) {
    assert.equal(isTileRenderable({ view: id, size: "s" }), true, id);
  }
  assert.equal(isTileRenderable({ view: "agent_hours_by_scope", size: "m" }), false, "phase 3's own view, not built yet");
  assert.equal(isTileRenderable({ view: "nonsense", size: "s" }), false, "not even in the vocabulary");
  assert.equal(isTileRenderable({ metric: "throughput_week", size: "s" }), false, "no metric tile renders yet");
  assert.equal(isTileRenderable(null), false);
  assert.equal(isTileRenderable({ view: "kpis", metric: "throughput_week", size: "s" }), false, "both is not a tile");
});

// ------------------------------------------------ dashboard.js renders it

// `modal.js`, which `tasks.js` (imported by `dashboard.js`) opens its
// confirmations in, listens for Escape on the document as it loads -- so
// there has to be one before dashboard.js is imported, the same
// requirement `operations.test.js`/`scenarios.test.js` document.
const bare = { addEventListener() {}, getElementById: () => null };
globalThis.document = bare;
const { renderDashboard, loadDashboard } = await import("../js/dashboard.js");

const served = readFileSync(new URL("../../crates/factory-daemon/src/ui.rs", import.meta.url), "utf8");

test("dashboard-model.js is served, so a live page can import it", () => {
  assert.match(served, /"js\/dashboard-model\.js"/);
});

test("renderDashboard renders the default tile list's sections, in order, from #dash", () => {
  const dashEl = { innerHTML: "", querySelector: () => null, querySelectorAll: () => [] };
  document.getElementById = (id) => (id === "dash" ? dashEl : null);

  state.scope = null;
  state.scopes = [{ name: "root", path: "root", agents: [] }];
  state.tasks = new Map();

  renderDashboard();

  const html = dashEl.innerHTML;
  // Every DEFAULT_DASHBOARD view leaves a distinctive mark in the rendered
  // page -- the same headings/classes the fixed page always had -- checked
  // here in the order the tile list names them, not the order a browser
  // might have painted them in before this refactor existed.
  const marks = [
    ['class="kpis"', "the KPI row"],
    ["<h3>Throughput", "the Throughput card"],
    ["<h3>On the line", "the On the line card"],
    ["<h3>The production year", "the Production year card"],
    ["<h3>By scope", "the By scope card"],
  ];
  let last = -1;
  for (const [needle, label] of marks) {
    const at = html.indexOf(needle);
    assert.ok(at !== -1, `${label} is missing (looked for ${JSON.stringify(needle)})`);
    assert.ok(at > last, `${label} rendered out of order`);
    last = at;
  }
});

// -------------------------------------------------- loadDashboard (#159)

const PRODUCTION_ANSWER = {
  status: "ok",
  data: { production: { bin: "day", buckets: [], daily: [], earliest_run: null } },
};

function fakeDashEls() {
  return { dash: { innerHTML: "", querySelector: () => null, querySelectorAll: () => [] }, "dash-source": { textContent: "" } };
}

test("loadDashboard fetches /api/dashboard for the selected scope, draws its tiles, and shows the source", async () => {
  const elements = fakeDashEls();
  document.getElementById = (id) => (id in elements ? elements[id] : null);

  const requested = [];
  globalThis.fetch = async (path) => {
    requested.push(path);
    if (path.startsWith("/api/production")) {
      return { status: 200, statusText: "OK", json: async () => PRODUCTION_ANSWER };
    }
    return {
      status: 200,
      statusText: "OK",
      json: async () => ({
        status: "ok",
        data: {
          kind: "dashboard",
          tiles: [{ view: "kpis", size: "xl" }, { view: "agent_hours_by_scope", size: "m" }],
          source: "demo",
        },
      }),
    };
  };
  state.scope = "demo";
  state.scopes = [{ name: "demo", path: "demo", agents: [] }];
  state.tasks = new Map();

  await loadDashboard();

  assert.ok(requested.includes("/api/dashboard?scope=demo"), requested.join(", "));
  assert.equal(elements["dash-source"].textContent, "layout from demo");
  assert.match(elements.dash.innerHTML, /class="kpis"/, "the renderable tile draws its own card");
  assert.match(
    elements.dash.innerHTML,
    /agent_hours_by_scope — not drawn yet/,
    "a view id phase 3 has not built yet gets a placeholder, not nothing"
  );

  delete globalThis.fetch;
});

test("no `dashboard:` anywhere (tiles: null, source: null) draws exactly DEFAULT_DASHBOARD and shows no source", async () => {
  const elements = fakeDashEls();
  document.getElementById = (id) => (id in elements ? elements[id] : null);

  globalThis.fetch = async (path) => {
    if (path.startsWith("/api/production")) {
      return { status: 200, statusText: "OK", json: async () => PRODUCTION_ANSWER };
    }
    return { status: 200, statusText: "OK", json: async () => ({ status: "ok", data: { kind: "dashboard", tiles: null, source: null } }) };
  };
  state.scope = null;
  state.scopes = [{ name: "root", path: "root", agents: [] }];
  state.tasks = new Map();

  await loadDashboard();

  assert.equal(elements["dash-source"].textContent, "", "null source shows nothing, not the word null");
  assert.match(elements.dash.innerHTML, /<h3>By scope/, "the last DEFAULT_DASHBOARD card is there");

  delete globalThis.fetch;
});

test("a failed /api/dashboard fetch falls back to DEFAULT_DASHBOARD, never a blank page", async () => {
  const elements = fakeDashEls();
  document.getElementById = (id) => (id in elements ? elements[id] : null);

  globalThis.fetch = async (path) => {
    if (path.startsWith("/api/production")) {
      return { status: 200, statusText: "OK", json: async () => PRODUCTION_ANSWER };
    }
    throw new Error("offline");
  };
  state.scope = null;
  state.scopes = [{ name: "root", path: "root", agents: [] }];
  state.tasks = new Map();

  await loadDashboard();

  assert.equal(elements["dash-source"].textContent, "");
  assert.match(elements.dash.innerHTML, /class="kpis"/);
  assert.match(elements.dash.innerHTML, /<h3>Throughput/);
  assert.match(elements.dash.innerHTML, /<h3>By scope/);

  delete globalThis.fetch;
  document.getElementById = () => null;
  state.scope = null;
});
