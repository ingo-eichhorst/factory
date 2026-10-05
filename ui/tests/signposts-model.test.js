import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { signpostObservations } from "../js/signposts-model.js";
import { inboxItems } from "../js/operations-model.js";

test("signposts preserve authored order as separate observations with no actions", () => {
  const fact = { at: "2026-10-05T12:00:00Z", triggered: [
    { scenario: "slow year", metric: "throughput_week", reason: "0 is below 999", task_id: "must-not-be-an-action" },
    { scenario: "expensive", metric: "cost_week", reason: "4 is above 2" },
  ] };
  const rows = signpostObservations(fact, "#scenarios");
  assert.deepEqual(rows.map((row) => row.title), ["slow year", "expensive"]);
  for (const row of rows) {
    assert.equal(row.observation, true);
    assert.deepEqual(row.actions, []);
    assert.equal(row.href, "#scenarios");
    assert.equal(row.task_id, undefined);
    assert.equal(row.run_id, undefined);
  }
  assert.deepEqual(inboxItems({ attention: rows }), [], "observations never become waiting work");
});
test("missing malformed and empty signpost facts do not invent observations", () => {
  for (const fact of [undefined, null, {}, { triggered: [] }, { triggered: "no" }, { triggered: [null, {}, { scenario: "x", metric: 9, reason: "x" }] }]) {
    assert.deepEqual(signpostObservations(fact), []);
  }
});
test("signpost reasons are retained as text for the renderer to escape", () => {
  const rows = signpostObservations({ triggered: [{ scenario: "<script>", metric: "cost_week", reason: "<img onerror=run()>" }] });
  assert.equal(rows[0].reason, "<img onerror=run()>");
});
test("Dashboard reads the independent fact endpoint and keeps it outside Inbox actions", () => {
  const source = readFileSync(new URL("../js/dashboard.js", import.meta.url), "utf8");
  assert.match(source, /api\("\/api\/signposts"\)/);
  assert.match(source, /neededEndpoints\(tiles\)\.operations \? Promise\.resolve\(\) : loadSignposts\(\)/);
  assert.match(source, /fetchDates\(\), loadSignposts\(\)/);
  const inbox = source.slice(source.indexOf("function inboxRows("), source.indexOf("export async function loadInbox("));
  assert.ok(!inbox.includes("signpostObservations("));
  const markup = readFileSync(new URL("../index.html", import.meta.url), "utf8");
  assert.match(markup, /id="dash-signposts"/);
  assert.match(markup, /id="inbox-signposts"/);
});
