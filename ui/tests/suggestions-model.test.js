import test from "node:test";
import assert from "node:assert/strict";

import {
  KINDS,
  STATES,
  kindLabel,
  stateLabel,
  isTerminal,
  canTask,
  canDismiss,
  canDone,
  canAsk,
  formatTokens,
  formatCost,
  groupSummaryLine,
  matchesQuery,
  filterByQuery,
  visibleGroups,
  suggestionById,
  groupSuggestions,
  shortId,
  dismissBody,
  taskBody,
  askBody,
  validDismissReason,
  validQuestion,
  suggestionTaskHref,
  suggestionRunHref,
} from "../js/suggestions-model.js";

// -------------------------------------------------------------- labels

test("kindLabel and stateLabel cover every documented value and fall back for an unknown one", () => {
  for (const k of KINDS) assert.ok(kindLabel(k).length > 0, k);
  for (const s of STATES) assert.ok(stateLabel(s).length > 0, s);
  assert.equal(kindLabel("bogus"), "bogus");
  assert.equal(stateLabel(""), "");
});

// ---------------------------------------------------------- state machine

test("isTerminal is true only for dismissed and done", () => {
  assert.equal(isTerminal("open"), false);
  assert.equal(isTerminal("tasked"), false);
  assert.equal(isTerminal("dismissed"), true);
  assert.equal(isTerminal("done"), true);
});

test("canTask/canDismiss/canDone follow the same open-or-tasked rule, canAsk also needs a session", () => {
  const open = { state: "open", session_id: "s1" };
  const tasked = { state: "tasked", session_id: "s1" };
  const dismissed = { state: "dismissed", session_id: "s1" };
  const done = { state: "done", session_id: "s1" };
  const noSession = { state: "open", session_id: null };

  for (const s of [open, tasked]) {
    assert.equal(canTask(s), true);
    assert.equal(canDismiss(s), true);
    assert.equal(canDone(s), true);
  }
  for (const s of [dismissed, done]) {
    assert.equal(canTask(s), false);
    assert.equal(canDismiss(s), false);
    assert.equal(canDone(s), false);
    assert.equal(canAsk(s), false, "a terminal suggestion offers no ask button either");
  }
  assert.equal(canAsk(open), true);
  assert.equal(canAsk(noSession), false, "nothing to resume without a recorded session");
});

// --------------------------------------------------------------- formatting

test("formatTokens shows small counts plainly and abbreviates thousands", () => {
  assert.equal(formatTokens(null), "?");
  assert.equal(formatTokens(undefined), "?");
  assert.equal(formatTokens(0), "0");
  assert.equal(formatTokens(500), "500");
  assert.equal(formatTokens(1500), "1.5k");
  assert.equal(formatTokens(25000), "25k");
});

test("formatCost is unknown for null/undefined, zero reads as free, and sub-dollar keeps more precision", () => {
  assert.equal(formatCost(null), null);
  assert.equal(formatCost(undefined), null);
  assert.equal(formatCost(0), "$0");
  assert.equal(formatCost(0.004), "$0.004");
  assert.equal(formatCost(1.2), "$1.20");
});

// ----------------------------------------------------------------- grouping

test("groupSummaryLine names the count, how many are open, and only the cost bits that exist", () => {
  const all = groupSummaryLine({ count: 3, open_count: 3, total_wasted_tokens: 0, total_cost_usd: 0 });
  assert.equal(all, "3 suggestions · all open");

  const mixed = groupSummaryLine({ count: 3, open_count: 1, total_wasted_tokens: 1500, total_cost_usd: 0.5 });
  assert.equal(mixed, "3 suggestions · 1 open · ~1.5k tokens claimed, $0.50 recorded");

  const one = groupSummaryLine({ count: 1, open_count: 1, total_wasted_tokens: 0, total_cost_usd: 0 });
  assert.equal(one, "1 suggestion · all open");
});

// ------------------------------------------------------------------ search

const SUGGESTIONS = [
  { id: "a", target: "L2 secret stripe_key", summary: "blocked web call", detail: null, scope: "demo", agent: "worker" },
  { id: "b", target: "L2 secret stripe_key", summary: "same again", detail: "outbound https", scope: "demo", agent: "other" },
  { id: "c", target: "the onboarding guide", summary: "stale screenshot", detail: null, scope: "other-scope", agent: "worker" },
];

test("matchesQuery matches target, summary, detail, scope or agent, case-insensitively", () => {
  assert.ok(matchesQuery(SUGGESTIONS[0], "stripe"));
  assert.ok(matchesQuery(SUGGESTIONS[0], "BLOCKED"));
  assert.ok(matchesQuery(SUGGESTIONS[1], "https"));
  assert.ok(matchesQuery(SUGGESTIONS[2], "other-scope"));
  assert.ok(!matchesQuery(SUGGESTIONS[0], "nothing here"));
  assert.ok(matchesQuery(SUGGESTIONS[0], ""), "an empty query matches everything");
  assert.ok(matchesQuery(SUGGESTIONS[0], "   "));
});

test("filterByQuery narrows the flat list", () => {
  assert.deepEqual(filterByQuery(SUGGESTIONS, "stripe").map((s) => s.id), ["a", "b"]);
  assert.deepEqual(filterByQuery(SUGGESTIONS, "").map((s) => s.id), ["a", "b", "c"]);
});

test("visibleGroups narrows each group's ids by the query and drops a group a query empties out entirely", () => {
  const report = {
    suggestions: SUGGESTIONS,
    groups: [
      { target: "L2 secret stripe_key", ids: ["a", "b"] },
      { target: "the onboarding guide", ids: ["c"] },
    ],
  };
  assert.deepEqual(visibleGroups(report, "").map((g) => g.target), ["L2 secret stripe_key", "the onboarding guide"]);
  const narrowed = visibleGroups(report, "stripe");
  assert.equal(narrowed.length, 1);
  assert.deepEqual(narrowed[0].ids, ["a", "b"]);
  const one = visibleGroups(report, "https");
  assert.deepEqual(one[0].ids, ["b"], "a query can narrow a group down to one of its members");
  assert.equal(visibleGroups(report, "onboarding").length, 1);
  assert.equal(visibleGroups(null, "x").length, 0);
});

test("visibleGroups never mutates the report it was handed", () => {
  const report = {
    suggestions: SUGGESTIONS,
    groups: [{ target: "L2 secret stripe_key", ids: ["a", "b"] }],
  };
  const before = JSON.parse(JSON.stringify(report));
  visibleGroups(report, "https");
  assert.deepEqual(report, before);
});

test("suggestionById and groupSuggestions look a suggestion up by id and preserve a group's own order", () => {
  const report = { suggestions: SUGGESTIONS, groups: [] };
  assert.equal(suggestionById(report, "b").summary, "same again");
  assert.equal(suggestionById(report, "missing"), null);
  assert.equal(suggestionById(null, "a"), null);
  const found = groupSuggestions(report, { ids: ["b", "a", "missing"] });
  assert.deepEqual(found.map((s) => s.id), ["b", "a"]);
});

// --------------------------------------------------------------------- misc

test("shortId truncates to eight characters and tolerates a missing id", () => {
  assert.equal(shortId("0123456789abcdef"), "01234567");
  assert.equal(shortId("abc"), "abc");
  assert.equal(shortId(null), "");
  assert.equal(shortId(undefined), "");
});

test("the request-body builders carry exactly the field the daemon expects, nothing else", () => {
  assert.deepEqual(dismissBody("not worth it"), { reason: "not worth it" });
  assert.deepEqual(taskBody(["a", "b"]), { ids: ["a", "b"] });
  assert.deepEqual(askBody("why did this fail?"), { question: "why did this fail?" });
});

test("validDismissReason/validQuestion refuse empty or whitespace-only text", () => {
  assert.equal(validDismissReason("a reason"), true);
  assert.equal(validDismissReason(""), false);
  assert.equal(validDismissReason("   "), false);
  assert.equal(validDismissReason(null), false);
  assert.equal(validQuestion("why?"), true);
  assert.equal(validQuestion(""), false);
  assert.equal(validQuestion(undefined), false);
});

// --------------------------------------------------------------------- links

test("suggestionTaskHref and suggestionRunHref draw the same task-modal link quality.js's taskHref would", () => {
  const taskHref = suggestionTaskHref("demo", "t1");
  assert.match(taskHref, /\/task\/t1(\/|$)/);
  const runHref = suggestionRunHref("demo", "t1", "r1");
  assert.match(runHref, /\/task\/t1\/r1$/);
});
