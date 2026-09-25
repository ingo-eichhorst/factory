import test from "node:test";
import assert from "node:assert/strict";

import { rowLanes, rowStyle, laneStyle, concurrency } from "../js/occupancy-model.js";

test("a row that never overlapped carries no lane geometry at all", () => {
  // Exactly the markup from before #120: no style on the row, none on a block.
  assert.equal(rowLanes({ lanes: 1 }), 1);
  assert.equal(rowStyle({ lanes: 1 }), "");
  assert.equal(laneStyle({ lane: 0 }), "");
  assert.equal(concurrency({ agent: "sh", live: 1 }), null);
  assert.equal(concurrency({ agent: "sh", live: 0 }), null);
});

test("a payload from a daemon without lanes is one lane", () => {
  assert.equal(rowLanes({}), 1);
  assert.equal(rowStyle({}), "");
  assert.equal(laneStyle({}), "");
  assert.equal(concurrency({ agent: "sh" }), null);
});

test("three concurrent runs stack into three lanes", () => {
  const row = { agent: "builder", lanes: 3, live: 3 };
  assert.equal(rowLanes(row), 3);
  assert.equal(rowStyle(row), ` style="--lanes:3"`);
  assert.deepEqual([0, 1, 2].map((lane) => laneStyle({ lane })), ["", "--lane:1;", "--lane:2;"]);
  assert.deepEqual(concurrency(row), { tag: "×3", title: "3 runs of builder are live at once" });
});

test("garbage in a lane field never becomes a style", () => {
  assert.equal(rowStyle({ lanes: "3;color:red" }), "");
  assert.equal(laneStyle({ lane: "1;color:red" }), "");
  assert.equal(laneStyle({ lane: -1 }), "");
});
