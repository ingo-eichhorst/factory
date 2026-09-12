import test from "node:test";
import assert from "node:assert/strict";

import {
  focusForMode,
  hallFrame,
  interiorPlacements,
  scopesForMode,
  siteFrame,
} from "../js/site-focus.js";

test("Render retains the whole site while Plan retains scoped filtering", () => {
  const scopes = [{ name: "one" }, { name: "two" }, { name: "three" }];
  const inScope = (name) => name === "two";

  assert.deepEqual(scopesForMode(scopes, 0, inScope).map((s) => s.name), ["two"]);
  assert.deepEqual(scopesForMode(scopes, 1, inScope).map((s) => s.name), ["one", "two", "three"]);
});

test("only an exact Render scope selection becomes a hall focus", () => {
  const two = { id: "two" };
  const byId = { two };

  assert.equal(focusForMode(byId, "two", 1), two);
  assert.equal(focusForMode(byId, "two", 0), null);
  assert.equal(focusForMode(byId, null, 1), null);
  assert.equal(focusForMode(byId, "gone", 1), null);
});

test("interior agents are distinct and remain inside the hall walls", () => {
  const hall = { x: 10, y: 20, w: 4, d: 3.4 };
  const placements = interiorPlacements(hall, 24);

  assert.equal(placements.length, 24);
  assert.equal(new Set(placements.map((p) => `${p.x}:${p.z}`)).size, 24);
  placements.forEach((p) => {
    assert.ok(p.x > hall.x && p.x < hall.x + hall.w);
    assert.ok(p.z > hall.y && p.z < hall.y + hall.d);
    assert.ok(p.scale > 0 && p.scale <= 1.4);
  });

  assert.deepEqual(interiorPlacements(hall, 0), []);
  const [one] = interiorPlacements(hall, 1);
  assert.equal(one.x, hall.x + hall.w / 2);
  assert.equal(one.z, hall.y + hall.d / 2);
});

test("focus frames the hall more closely and clearing it restores the site", () => {
  const scene = { ZONE: { x: -2, y: -2, w: 30, d: 20 } };
  const hall = { x: 12, y: 4, w: 4, d: 3.4 };
  const site = siteFrame(scene);
  const focus = hallFrame(hall);

  assert.deepEqual([site.tx, site.tz], [13, 8]);
  assert.deepEqual([focus.tx, focus.tz], [14, 5.7]);
  assert.ok(focus.r < site.r);
  assert.ok(focus.pol < site.pol);
});
