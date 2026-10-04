import test from "node:test";
import assert from "node:assert/strict";

globalThis.document = { addEventListener() {}, getElementById: () => null };
const { agentTags } = await import("../js/agents.js");

test("roster tags keep readiness and omit empty tags without changing spacing", () => {
  const worker = { role: "worker", lifetime: "task", sandbox: "none", source: "builtin", declared: true };
  assert.equal(agentTags(worker), "");
  assert.equal(agentTags({ ...worker, is_default: true }), '<span class="tag">default</span>');
  const checking = agentTags({ ...worker, sandbox: "openshell" });
  assert.equal(checking, '<span class="tag ok">openshell</span> <span class="tag">checking</span>');
  const needs = agentTags({ ...worker, sandbox: "openshell", readiness: { state: "needs", thing: '<token> "missing"' } });
  assert.match(needs, /data-tone="bad"/);
  assert.match(needs, /&lt;token&gt;/);
  assert.doesNotMatch(needs, /<token>/);
});
