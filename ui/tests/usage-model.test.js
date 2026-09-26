import test from "node:test";
import assert from "node:assert/strict";

import { fmtUsd, fmtTokens, totalTokens, runUsageView, taskUsageLine, costFigure } from "../js/usage-model.js";

test("an unknown count is a question mark and a tiny cost is not free", () => {
  assert.equal(fmtTokens(null), "?");
  assert.equal(fmtTokens(61_000), "61.0k");
  assert.equal(fmtTokens(1_250_000), "1.3M");
  assert.equal(fmtUsd(0.001), "<$0.01");
  assert.equal(fmtUsd(2.7), "$2.70");
  assert.equal(fmtUsd(null), "?");
  assert.equal(totalTokens({ input: 1, output: 2, cache_read: 3, cache_write: null }), null);
  assert.equal(totalTokens({ input: 1, output: 2, cache_read: 3, cache_write: 4 }), 10);
});

test("a run with no usage, unknown usage and measured usage each read honestly", () => {
  assert.equal(runUsageView(null).tone, "none");
  const unknown = runUsageView({ state: "unknown", reason: "the herdr runtime has no source for usage" });
  assert.equal(unknown.tone, "unknown");
  assert.match(unknown.headline, /no source for usage/);

  const known = runUsageView({
    state: "known", partial: true,
    tokens: { input: 61000, output: 900, cache_read: null, cache_write: 0 },
    cost_usd: 2.7, pricing_sources: ["litellm@x"], models: ["gpt-5"], sessions: 1,
    as_of: "2026-09-25T12:00:00Z", as_of_point: "turn_ended", notes: ["tokens.cache_read is unknown for session s1"],
  });
  assert.equal(known.tone, "known");
  assert.equal(known.headline, "at least ? tokens · $2.70", "one unknown type makes the total unknown");
  assert.ok(known.lines[0].includes("? cache read"));
  assert.ok(known.lines.includes("as of turn ended"));
  assert.deepEqual(known.notes, ["tokens.cache_read is unknown for session s1"]);
});

test("a task's line says which runs are not in its sum", () => {
  assert.equal(taskUsageLine(null), null);
  assert.equal(taskUsageLine({ runs: 0 }), null);
  const line = taskUsageLine({
    runs: 3, runs_unknown: 1, runs_cost_unknown: 0, runs_partial: 1,
    tokens: { input: 1000, output: 500, cache_read: 0, cache_write: 0 }, cost_usd: 0.42,
  });
  assert.equal(line, "All 3 runs: 1.5k tokens · $0.42 (not in the sum: 1 unmeasured, 1 partial)");
  assert.equal(taskUsageLine({ runs: 2, runs_unknown: 2, tokens: {}, cost_usd: 0 }), "All 2 runs: usage unknown");
  assert.equal(
    taskUsageLine({ runs: 1, runs_unknown: 0, runs_cost_unknown: 1, tokens: { input: 10 }, cost_usd: 0 }),
    "All 1 run: 10 tokens · ? (not in the sum: 1 without a cost)",
    "tokens but no measured cost is ?, not free"
  );
});

test("costFigure: a row whose runs are all usage-unknown reads 'unknown', with no bar to draw -- never a measured-looking $0.00", () => {
  const allUnknown = { key: "demo", runs: 4, runs_unknown: 4, runs_partial: 0, runs_cost_unknown: 0, cost_usd: 0.0, tokens: {} };
  assert.deepEqual(costFigure(allUnknown), { text: "unknown", hasCost: false });
});

test("costFigure: no runs at all reads a bare dash, also with no bar", () => {
  assert.deepEqual(costFigure({ runs: 0 }), { text: "—", hasCost: false });
  assert.deepEqual(costFigure(null), { text: "—", hasCost: false });
});

test("costFigure: every run costed, none missing, reads the plain figure with no lower-bound prefix and a real bar", () => {
  const full = { key: "demo", runs: 3, runs_unknown: 0, runs_partial: 0, runs_cost_unknown: 0, cost_usd: 9.5, tokens: {} };
  assert.deepEqual(costFigure(full), { text: "$9.50", hasCost: true });
});

test("costFigure: some runs unmeasured, uncosted or partial reads a lower bound, prefixed ≥, still with a bar (it is a real, if partial, figure)", () => {
  const partiallyUnmeasured = { key: "demo", runs: 4, runs_unknown: 1, runs_partial: 0, runs_cost_unknown: 0, cost_usd: 6.0, tokens: {} };
  assert.deepEqual(costFigure(partiallyUnmeasured), { text: "≥ $6.00", hasCost: true });

  const someUncosted = { key: "demo", runs: 4, runs_unknown: 0, runs_partial: 0, runs_cost_unknown: 2, cost_usd: 3.25, tokens: {} };
  assert.deepEqual(costFigure(someUncosted), { text: "≥ $3.25", hasCost: true });

  const somePartial = { key: "demo", runs: 2, runs_unknown: 0, runs_partial: 1, runs_cost_unknown: 0, cost_usd: 1.1, tokens: {} };
  assert.deepEqual(costFigure(somePartial), { text: "≥ $1.10", hasCost: true });
});

test("costFigure: nothing costed but not every run is usage-unknown (all excluded some other way) reads ?, with no bar", () => {
  const nothingCosted = { key: "demo", runs: 2, runs_unknown: 0, runs_partial: 0, runs_cost_unknown: 2, cost_usd: 0, tokens: { input: 10 } };
  assert.deepEqual(costFigure(nothingCosted), { text: "?", hasCost: false });
});
