import test from "node:test";
import assert from "node:assert/strict";

import { canStart, instanceSummary, instancesOf, liveIds, scalePayload } from "../js/agents-model.js";

const three = {
  name: "claude", lifetime: "permanent", id: "root/claude", state: "ready", wanted: 3,
  instances: [
    { id: "root/claude", instance: 1, state: "ready" },
    { id: "root/claude#2", instance: 2, state: "starting" },
    { id: "root/claude#3", instance: 3, state: "gone" },
  ],
};

test("a standing agent with several instances says how many are up of how many it keeps", () => {
  assert.deepEqual(instanceSummary(three), { wanted: 3, up: 2, several: true, label: "instances 2/3 up" });
  assert.equal(canStart(three), true, "one of three is down");
  assert.deepEqual(liveIds(three), ["root/claude#2", "root/claude"], "Stop takes the highest first");
});

test("one standing session is the row it always was", () => {
  const one = { name: "watcher", lifetime: "permanent", id: "demo/watcher", state: "ready", wanted: 1,
    instances: [{ id: "demo/watcher", instance: 1, state: "ready" }] };
  assert.equal(instanceSummary(one).several, false);
  assert.equal(canStart(one), false);
  // A daemon from before instances sends neither `wanted` nor `instances`.
  const old = { name: "watcher", lifetime: "temporary", id: "demo/watcher", state: "stopped" };
  assert.deepEqual(instancesOf(old).map(i => i.id), ["demo/watcher"]);
  assert.deepEqual(instanceSummary(old), { wanted: 1, up: 0, several: false, label: "instances 0/1 up" });
  assert.equal(canStart(old), true);
});

test("a draining surplus instance still shows, and still counts as up", () => {
  const draining = { ...three, wanted: 1, instances: [
    { id: "root/claude", instance: 1, state: "ready" },
    { id: "root/claude#2", instance: 2, state: "ready", surplus: true },
  ] };
  assert.deepEqual(instanceSummary(draining), { wanted: 1, up: 2, several: true, label: "instances 2/1 up" });
});

test("a task agent keeps no standing session", () => {
  assert.equal(instanceSummary({ name: "codex", lifetime: "task" }), null);
  assert.equal(canStart({ name: "codex", lifetime: "task" }), false);
  assert.deepEqual(liveIds({ name: "codex", lifetime: "task" }), []);
});

test("− and + send one relative step, never below one instance", () => {
  assert.deepEqual(scalePayload(" root ", "claude", 1, 3), { scope: "root", name: "claude", by: 1 });
  assert.deepEqual(scalePayload("root", "claude", -1, 3), { scope: "root", name: "claude", by: -1 });
  assert.throws(() => scalePayload("root", "claude", -1, 1), /at least one instance/);
  assert.throws(() => scalePayload("", "claude", 1, 1), /standing agent/);
  assert.throws(() => scalePayload("root", "claude", 0, 1), /whole number/);
});
