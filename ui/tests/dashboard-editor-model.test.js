import test from "node:test";
import assert from "node:assert/strict";

import { VIEW_IDS, DEFAULT_DASHBOARD } from "../js/dashboard-model.js";
import {
  VIEW_LABELS,
  buildCatalogue,
  searchCatalogue,
  entryToTile,
  seedTiles,
  addTile,
  removeTile,
  moveTileUp,
  moveTileDown,
  setTileSize,
  editorErrors,
  canReset,
} from "../js/dashboard-editor-model.js";

// ------------------------------------------------------------- catalogue

test("the catalogue has one entry per view id, labelled, plus one per registry metric", () => {
  const registry = [
    { id: "throughput_week", title: "Throughput (week)", description: "runs finished this week" },
    { id: "unit_cost", title: "Unit cost", description: "" },
  ];
  const catalogue = buildCatalogue(registry);
  assert.equal(catalogue.length, VIEW_IDS.length + 2);
  for (const id of VIEW_IDS) {
    const entry = catalogue.find((e) => e.kind === "view" && e.id === id);
    assert.ok(entry, `${id} is in the catalogue`);
    assert.equal(entry.title, VIEW_LABELS[id]);
  }
  const metric = catalogue.find((e) => e.kind === "metric" && e.id === "throughput_week");
  assert.equal(metric.title, "Throughput (week)");
  assert.equal(metric.subtitle, "runs finished this week");
});

test("every view label is used and none are stray", () => {
  assert.deepEqual(Object.keys(VIEW_LABELS).sort(), [...VIEW_IDS].sort());
});

test("a missing, undefined or empty registry still gives every view tile", () => {
  for (const registry of [undefined, null, []]) {
    const catalogue = buildCatalogue(registry);
    assert.equal(catalogue.length, VIEW_IDS.length);
    assert.ok(catalogue.every((e) => e.kind === "view"));
  }
});

test("a metric with no title or description falls back to its id and a generic subtitle", () => {
  const catalogue = buildCatalogue([{ id: "bare_metric" }]);
  const metric = catalogue.find((e) => e.id === "bare_metric");
  assert.equal(metric.title, "bare_metric");
  assert.equal(metric.subtitle, "metric");
});

test("search is case-insensitive over id and title, and a blank query returns everything", () => {
  const catalogue = buildCatalogue([{ id: "unit_cost", title: "Unit cost" }]);
  assert.deepEqual(searchCatalogue(catalogue, ""), catalogue);
  assert.deepEqual(searchCatalogue(catalogue, "   "), catalogue);

  const byTitle = searchCatalogue(catalogue, "COST");
  assert.ok(byTitle.some((e) => e.id === "unit_cost"));
  assert.ok(!byTitle.some((e) => e.id === "kpis"));

  const byId = searchCatalogue(catalogue, "kpis");
  assert.deepEqual(byId.map((e) => e.id), ["kpis"]);

  assert.deepEqual(searchCatalogue(catalogue, "no such thing"), []);
});

test("a catalogue entry becomes exactly one of the two tile shapes", () => {
  assert.deepEqual(entryToTile({ kind: "view", id: "kpis" }, "xl"), { view: "kpis", size: "xl" });
  assert.deepEqual(entryToTile({ kind: "metric", id: "unit_cost" }, "s"), { metric: "unit_cost", size: "s" });
});

// ------------------------------------------------------------------ seeding

test("seedTiles copies the fetched layout rather than aliasing it", () => {
  const fetched = [{ view: "kpis", size: "xl" }];
  const seeded = seedTiles(fetched);
  assert.deepEqual(seeded, fetched);
  seeded[0].size = "s";
  assert.equal(fetched[0].size, "xl", "the fetched array is untouched");
});

test("seedTiles falls back to a mutable copy of DEFAULT_DASHBOARD, which stays frozen", () => {
  for (const empty of [null, undefined, []]) {
    const seeded = seedTiles(empty);
    assert.deepEqual(seeded, DEFAULT_DASHBOARD);
    // DEFAULT_DASHBOARD is frozen two levels deep; editing the seeded copy
    // must not throw, and must not mutate the frozen source.
    seeded.push({ view: "cost", size: "s" });
    seeded[0].size = "s";
    assert.equal(DEFAULT_DASHBOARD.length, 5);
    assert.equal(DEFAULT_DASHBOARD[0].size, "xl");
    assert.ok(Object.isFrozen(DEFAULT_DASHBOARD[0]));
  }
});

// -------------------------------------------------------------- list edits

test("addTile appends at the end without touching the rest of the list", () => {
  const tiles = [{ view: "kpis", size: "xl" }];
  const next = addTile(tiles, { kind: "metric", id: "unit_cost" }, "m");
  assert.deepEqual(next, [{ view: "kpis", size: "xl" }, { metric: "unit_cost", size: "m" }]);
  assert.equal(tiles.length, 1, "the input is not mutated");
});

test("addTile defaults to size m when none is given", () => {
  const next = addTile([], { kind: "view", id: "cost" });
  assert.deepEqual(next, [{ view: "cost", size: "m" }]);
});

test("removeTile drops exactly the tile at that index", () => {
  const tiles = [{ view: "kpis", size: "xl" }, { view: "cost", size: "s" }, { view: "inbox", size: "m" }];
  const next = removeTile(tiles, 1);
  assert.deepEqual(next, [{ view: "kpis", size: "xl" }, { view: "inbox", size: "m" }]);
  assert.equal(tiles.length, 3, "the input is not mutated");
});

test("removeTile out of range is a harmless no-op copy", () => {
  const tiles = [{ view: "kpis", size: "xl" }];
  assert.deepEqual(removeTile(tiles, 5), tiles);
  assert.deepEqual(removeTile(tiles, -1), tiles);
});

test("moveTileUp/moveTileDown swap with the neighbour, and stop at the edges", () => {
  const tiles = [{ view: "kpis", size: "xl" }, { view: "cost", size: "s" }, { view: "inbox", size: "m" }];

  const up = moveTileUp(tiles, 1);
  assert.deepEqual(up.map((t) => t.view), ["cost", "kpis", "inbox"]);
  assert.deepEqual(tiles.map((t) => t.view), ["kpis", "cost", "inbox"], "the input is not mutated");

  const down = moveTileDown(tiles, 1);
  assert.deepEqual(down.map((t) => t.view), ["kpis", "inbox", "cost"]);

  assert.deepEqual(moveTileUp(tiles, 0), tiles, "already first: no-op");
  assert.deepEqual(moveTileDown(tiles, tiles.length - 1), tiles, "already last: no-op");
  assert.deepEqual(moveTileUp(tiles, 99), tiles, "out of range: no-op");
});

test("setTileSize changes only the named tile, and refuses an unknown size", () => {
  const tiles = [{ view: "kpis", size: "xl" }, { view: "cost", size: "s" }];
  const next = setTileSize(tiles, 1, "l");
  assert.deepEqual(next, [{ view: "kpis", size: "xl" }, { view: "cost", size: "l" }]);
  assert.equal(tiles[1].size, "s", "the input is not mutated");

  assert.deepEqual(setTileSize(tiles, 1, "huge"), tiles, "an unknown size is refused, not written in");
  assert.deepEqual(setTileSize(tiles, 9, "s"), tiles, "out of range is a no-op too");
});

// -------------------------------------------------------------- validation

test("editorErrors is validateDashboard's own answer", () => {
  assert.deepEqual(editorErrors([]), ["a dashboard needs at least one tile"]);
  assert.deepEqual(editorErrors([{ view: "kpis", size: "xl" }]), []);
  assert.deepEqual(editorErrors([{ view: "not-a-real-view", size: "xl" }]), [
    `tile 0: unknown view tile "not-a-real-view"`,
  ]);
});

// ------------------------------------------------------------------ reset

test("canReset is true only when the scope being edited is the one currently answering", () => {
  assert.equal(canReset("demo", "demo"), true);
  assert.equal(canReset("projects", "demo"), false, "demo inherits projects' layout; nothing of its own to reset");
  assert.equal(canReset(null, "demo"), false, "the built-in default has no block anywhere to remove");
  assert.equal(canReset("demo", null), false, "nothing selected to edit");
});
