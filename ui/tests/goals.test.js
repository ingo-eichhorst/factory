import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { state } from "../js/core.js";
import { readHash, setRouter } from "../js/scopes.js";

// `modal.js`, which the view opens its check-in form in, listens for Escape
// on the document as it loads -- so there has to be one before the view is
// imported, the same requirement `policy.test.js` documents.
const bare = { addEventListener() {}, getElementById: () => null };
globalThis.document = bare;
const { loadGoals, renderGoals } = await import("../js/goals.js");

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const wiring = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");

test("L6 Direction's Goals tab exists, is ordered before Policy, and is wired to its loader", () => {
  assert.match(page, /id="tab-goals"[^>]*>Goals<\/button>/);
  assert.match(page, /id="view-goals"/);
  const tabGoals = page.indexOf('id="tab-goals"');
  const tabPolicy = page.indexOf('id="tab-policy"');
  assert.ok(tabGoals > 0 && tabPolicy > 0 && tabGoals < tabPolicy, "Goals precedes Policy in the view-tab row");
  // Not anchored at the closing bracket: `dir` grows a trailing "scenarios"
  // (#100) after this pair, and Quality (#107) sits between the two; this
  // assertion only cares that Goals still precedes Policy in it.
  assert.match(wiring, /dir:\s*\["goals",\s*"quality",\s*"policy"/);
  assert.match(wiring, /goals:\s*\{\s*onShow:\s*loadGoals\s*\}/);
});

test("a goals_changed event reloads the tab through the same loader the refresh button uses", () => {
  assert.match(wiring, /ev\.type === "goals_changed"/);
  assert.match(wiring, /reloadGoals\(\)/);
});

test("a link with a level segment and an old bare link both land on the Goals page", () => {
  setRouter({ pages: ["dashboard", "goals", "policy"], redirects: {} });

  globalThis.location = { hash: "#all/dir/goals" };
  assert.equal(readHash().page, "goals");
  delete globalThis.location;

  globalThis.location = { hash: "#all/goals" };
  assert.equal(readHash().page, "goals");
  delete globalThis.location;
});

// A real `GET /api/goals` answer's `.report`, trimmed -- see
// `goals-model.test.js`'s own header comment for how it was captured.
function fixtureReport() {
  return {
    direction: {
      vision: "Every product this company ships can prove it was done well.",
      mission: "Run agents and let the operating data carry the evidence.",
      values: ["Evidence over assertion"],
      north_star: { metric: "first_pass_yield", why: "The clearest single signal." },
      inputs: ["throughput_week"],
      obstacles: [],
    },
    cycles: [{ id: "2026-q3", from: "2026-07-01", to: "2026-09-30", status: "current", score: 0.5 }],
    report: {
      cycle_id: "2026-q3",
      status: "current",
      elapsed: 0.9,
      objectives: [
        {
          objective: "ship-compliant",
          title: "Every product can ship CRA-compliant",
          score: 0.5,
          key_results: [
            {
              kr: "ship-compliant/cra-open-zero",
              title: "No open CRA controls",
              kind: "committed",
              manual: false,
              metric: "compliance.cra",
              baseline: 0,
              target: 1,
              value: 0.5,
              score: 0.5,
              band: "yellow",
              on_pace: true,
              source: "metric compliance.cra (as of 2026-09-25)",
              reasons: [],
              confidence: null,
            },
          ],
        },
      ],
    },
    findings: [{ kind: "too_many_objectives", subject: "2026-q3.yaml", detail: "six objectives" }],
    north_star: { metric: "first_pass_yield", why: "…", value: { id: "first_pass_yield", value: 1.0, as_of: "2026-09-25T00:00:00Z" } },
    inputs: [{ metric: "throughput_week", value: { id: "throughput_week", value: 5.0, as_of: "2026-09-25T00:00:00Z" } }],
    roadmap: [{ id: "x", title: "X", lane: "now", objectives: ["ship-compliant"], why: "why" }],
    checkins: {},
  };
}

function fixtureMetrics() {
  return {
    kind: "metrics",
    values: [],
    series: [],
    registry: [{ id: "compliance.cra", title: "Compliance share (cra)", description: "…", unit: "ratio", better: "higher", source: "…", available: true, unavailable_reason: null }],
  };
}

function fakeElements() {
  const el = (extra) => ({ innerHTML: "", textContent: "", hidden: false, querySelectorAll: () => [], ...extra });
  return {
    "goals-scope-note": el(),
    "goals-error": el(),
    "goals-empty": el(),
    "goals-body": el(),
    "goal-vision": el(),
    "goal-mission": el(),
    "goal-values": el(),
    "goal-cycle-select": el(),
    "goals-cycle-note": el(),
    "goal-progress": el(),
    "goal-view-seg": el(),
    "goal-view-map": el(),
    "goal-view-roadmap": el(),
    "goal-view-orbit": el(),
    "goal-view-table": el(),
    "goal-map": el(),
    "goal-legend": el(),
    "goal-layers-wrap": null,
    "goal-connectors": null,
    "goal-roadmap": el(),
    "goal-orbit": el(),
    "goal-table": el(),
    "goal-findings": el(),
    "goal-no-findings": el(),
  };
}

test("loadGoals fetches the selected scope and cycle, then a second /api/metrics call for the ids the report names", async () => {
  const elements = fakeElements();
  globalThis.document = { ...bare, getElementById: (id) => (id in elements ? elements[id] : null) };

  const requested = [];
  globalThis.fetch = async (path) => {
    requested.push(path);
    if (path.startsWith("/api/metrics")) {
      return { status: 200, statusText: "OK", json: async () => ({ status: "ok", data: fixtureMetrics() }) };
    }
    return { status: 200, statusText: "OK", json: async () => ({ status: "ok", data: { kind: "goals", report: fixtureReport() } }) };
  };
  state.scope = "demo";
  state.goalsCycle = null;

  await loadGoals();

  assert.equal(requested[0], "/api/goals?scope=demo");
  assert.match(requested[1], /^\/api\/metrics\?ids=/);
  assert.match(requested[1], /compliance\.cra/);
  assert.match(requested[1], /throughput_week/, "the three production metrics are always asked for");

  assert.match(elements["goal-vision"].textContent, /prove it was done well/);
  assert.match(elements["goal-mission"].textContent, /operating data/);
  assert.match(elements["goal-values"].innerHTML, /Evidence over assertion/);
  assert.match(elements["goal-map"].innerHTML, /Every product can ship CRA-compliant/);
  assert.match(elements["goal-map"].innerHTML, /No open CRA controls/);
  assert.match(elements["goal-table"].innerHTML, /No open CRA controls/);
  assert.match(elements["goal-roadmap"].innerHTML, />X<\/h4>/);
  assert.match(elements["goal-findings"].innerHTML, /six objectives/);
  assert.equal(elements["goals-body"].hidden, false);
  assert.equal(elements["goals-empty"].hidden, true);

  delete globalThis.fetch;
});

test("loadGoals shows the empty state when no direction is authored, and the error banner on a failed fetch", async () => {
  const elements = fakeElements();
  globalThis.document = { ...bare, getElementById: (id) => (id in elements ? elements[id] : null) };

  globalThis.fetch = async () => ({
    status: 200,
    statusText: "OK",
    json: async () => ({ status: "ok", data: { kind: "goals", report: { direction: null, cycles: [], report: null, findings: [], inputs: [], roadmap: [], checkins: {} } } }),
  });
  await loadGoals();
  assert.equal(elements["goals-empty"].hidden, false);
  assert.equal(elements["goals-body"].hidden, true);

  globalThis.fetch = async () => ({ status: 400, statusText: "Bad Request", json: async () => ({ status: "error", message: "no such scope: gone" }) });
  await loadGoals();
  assert.equal(elements["goals-error"].textContent, "no such scope: gone");
  assert.equal(elements["goals-error"].hidden, false);

  delete globalThis.fetch;
  globalThis.document = bare;
  state.scope = null;
  renderGoals(); // no throw with document stubbed back to the bare shim
});
