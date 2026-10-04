import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { GROUPS, budgetUrl, usd, uncertainty, spendRows, budgetCard, burnDown, budgetEvent } from "../js/budget-model.js";

const month = { from: "2026-10-01T00:00:00Z", until: "2026-11-01T00:00:00Z", as_of: "2026-10-16T12:00:00Z" };
const row = { key: "scope=work", runs: 1, runs_unknown: 0, runs_partial: 0, runs_cost_unknown: 0,
  runs_tokens_incomplete: 0, tokens: { input: 100, output: 20, cache_read: 30, cache_write: 5 }, cost_usd: 25 };
const card = { scope: "work", id: "stable-id", relation: "selected", monthly_usd: 50, spent: row,
  daily: [{ day: "2026-10-02", spent: {cost_usd: 25} }],
  assessment: {state: "within", remaining_usd: 25, projected_month_usd: 50, used_fraction: 0.5 } };

test("Budget exposes all requested groupings and encodes the scope rail", () => {
  assert.deepEqual(GROUPS.map(g => g[0]), ["scope", "agent", "issue", "workflow", "provider"]);
  assert.equal(budgetUrl(null), "/api/budget?group_by=scope");
  const parsed = new URL(budgetUrl("work & sibling", "workflow"), "http://example");
  assert.equal(parsed.searchParams.get("scope"), "work & sibling");
  assert.equal(parsed.searchParams.get("group_by"), "workflow");
});

test("Unknown is never formatted as free, even when known sums are zero", () => {
  assert.equal(usd(null), "unknown"); assert.equal(usd(NaN), "unknown");
  assert.equal(usd(0), "$0.00");
  const unknown = {...row, runs_unknown: 1, cost_usd: 0};
  const shaped = spendRows({rows: [unknown]})[0];
  assert.equal(shaped.cost, "unknown"); assert.equal(shaped.tokenText, "unknown");
  assert.match(shaped.uncertainty, /unmeasured/);
  assert.match(uncertainty({...row, runs_partial: 1, runs_cost_unknown: 1, runs_tokens_incomplete: 1}, 2), /partial.*without USD.*incomplete tokens.*unattributed/);
  assert.equal(spendRows({rows: [row]})[0].tokenText, "155 known");
});

test("Cards use server verdicts, distinguish absent from zero, and warn about ancestor caps", () => {
  assert.equal(budgetCard(card).status, "Within limit");
  assert.equal(budgetCard(card).usedPercent, 50);
  assert.equal(budgetCard({...card, monthly_usd: null}).limit, "not authored");
  assert.equal(budgetCard({...card, monthly_usd: 0}).limit, "$0.00");
  const unknown = budgetCard({...card, assessment: {state: "unknown"}});
  assert.equal(unknown.remaining, "unknown"); assert.equal(unknown.usedPercent, null);
  assert.match(budgetCard({...card, relation: "ancestor"}).ancestorNote, /outside the selected subtree/);
  assert.equal(budgetCard({...card, assessment: {state: "over", used_fraction: 3}}).usedPercent, 100);
});

test("Burn-down uses recorded daily spend, not an invented history or price", () => {
  const chart = burnDown(card, month, 310, 80);
  assert.equal(chart.points, "0,0.00 20.00,40.00 155.00,40.00");
  assert.equal(chart.planned, "0,0.00 310,80");
  assert.equal(burnDown({...card, assessment: {state: "unknown"}}, month), null);
  assert.equal(burnDown({...card, monthly_usd: 0}, month), null);
  assert.equal(burnDown(card, {...month, until: "not a date"}), null);
});

test("Only spend-changing events refresh; no default dashboard Budget fetch", () => {
  assert.equal(budgetEvent({type: "run_updated"}), true);
  assert.equal(budgetEvent({type: "task_deleted"}), true);
  assert.equal(budgetEvent({type: "agent_activity"}), false);
  const app = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");
  const html = readFileSync(new URL("../index.html", import.meta.url), "utf8");
  assert.match(app, /budget: \{ onShow: loadBudget \}/);
  assert.match(app, /dir: \["goals", "budget", "policy", "scenarios"\]/);
  assert.match(app, /state.tab === "budget" && budgetEvent/);
  assert.match(html, /id="tab-budget" hidden/);
  assert.match(html, /id="view-budget" hidden/);
  assert.match(html, /never sum them/);
});
