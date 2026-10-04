import test from "node:test";
import assert from "node:assert/strict";
import { state } from "../js/core.js";
import { loadBudget, wireBudget } from "../js/budget.js";

test("Budget fetches the selected scope, escapes labels, rejects stale answers and hides verdicts on failure", async () => {
  const nodes = new Map();
  globalThis.document = {getElementById(id) {
    if (!nodes.has(id)) nodes.set(id, {textContent: "", innerHTML: "", hidden: false, value: "scope"});
    return nodes.get(id);
  }};
  const pending = [];
  globalThis.fetch = url => new Promise(resolve => pending.push({url, resolve}));
  const response = scope => ({json: async () => ({status: "ok", data: {report: {
    month: {from: "2026-10-01T00:00:00Z", until: "2026-11-01T00:00:00Z", as_of: "2026-10-04T00:00:00Z"},
    catalogue: "/tmp/limits.yaml", findings: [], budgets: [{scope, id: "stable", path: "projects/work", relation: "selected", monthly_usd: 50,
      spent: {cost_usd: 0}, assessment: {state: "unknown", reason: "unmeasured"}}],
    spend: {total: {runs: 1, cost_usd: 0, runs_unknown: 1}, rows: [{key: scope, runs: 1, cost_usd: 0, runs_unknown: 1, runs_cost_unknown: 0, tokens: {}}]},
  }}})});
  try {
    wireBudget();
    assert.match(nodes.get("budget-group").innerHTML, /Provider account/);
    state.scope = "old"; const old = loadBudget();
    state.scope = "new & scope"; const latest = loadBudget();
    assert.match(pending[1].url, /scope=new\+%26\+scope/);
    pending[1].resolve(response('<img src=x onerror="bad">'));
    await latest;
    pending[0].resolve(response("old")); await old;
    assert.match(nodes.get("budget-cards").innerHTML, /&lt;img/);
    assert.doesNotMatch(nodes.get("budget-cards").innerHTML, /<img|>old</);
    assert.match(nodes.get("budget-spend").innerHTML, /unknown/);
    assert.equal(nodes.get("budget-body").hidden, false);
    assert.equal(nodes.get("budget-loading").hidden, true);
    globalThis.fetch = async () => ({json: async () => ({status: "error", message: "limits.yaml is malformed"})});
    await loadBudget();
    assert.equal(nodes.get("budget-body").hidden, true, "a safe old verdict must not survive a failed read");
    assert.equal(nodes.get("budget-error").hidden, false);
    assert.match(nodes.get("budget-error").textContent, /malformed/);
  } finally { state.scope = null; delete globalThis.fetch; delete globalThis.document; }
});
