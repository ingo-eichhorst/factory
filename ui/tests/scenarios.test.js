import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { state } from "../js/core.js";
import { readHash, setRouter } from "../js/scopes.js";

// `modal.js`, which the view opens its promote and matrix-cell forms in,
// listens for Escape on the document as it loads -- so there has to be one
// before the view is imported, the same requirement `policy.test.js`/
// `goals.test.js` document.
const bare = { addEventListener() {}, getElementById: () => null };
globalThis.document = bare;
const { loadScenarios, renderScenarios } = await import("../js/scenarios.js");

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const wiring = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");

test("L6 Direction's Scenarios tab exists, is ordered after Policy, and is wired to its loader", () => {
  assert.match(page, /id="tab-scenarios"[^>]*>Scenarios<\/button>/);
  assert.match(page, /id="view-scenarios"/);
  const tabPolicy = page.indexOf('id="tab-policy"');
  const tabScenarios = page.indexOf('id="tab-scenarios"');
  assert.ok(tabPolicy > 0 && tabScenarios > 0 && tabPolicy < tabScenarios, "Policy precedes Scenarios in the view-tab row");
  // Not anchored at the closing bracket: `dir` may grow another tab after
  // this one, the same restraint `goals.test.js`/`policy.test.js` now take
  // on their own copies of this assertion.
  assert.match(wiring, /dir:\s*\["goals",\s*"policy",\s*"scenarios"/);
  assert.match(wiring, /scenarios:\s*\{\s*onShow:\s*loadScenarios\s*\}/);
  // Never mixed into one number, in the page's own words (the issue's
  // guardrail, restated where a person actually reads the tab).
  assert.match(page, /never a single\s+line/);
});

test("policy_changed and goals_changed both reload the tab through the same loader", () => {
  assert.match(wiring, /\(ev\.type === "policy_changed" \|\| ev\.type === "goals_changed"\)[\s\S]{0,40}state\.tab === "scenarios"/);
  assert.match(wiring, /reloadScenarios\(\)/);
});

// A trimmed real `GET /api/scenarios` answer -- baseline plus the
// `ai-act-2027` scenario only (`capacity-drop` and the two narrative
// scenarios are cut for size; `scenarios-model.test.js`'s own `REAL_REPORT`
// keeps all four for the pure-function tests). Captured from a throwaway
// daemon on 127.0.0.1:8806 the way `scenarios-model.test.js`'s header
// comment describes; `per_week` trimmed to two points.
const REAL_REPORT = {
  scope: "dev-scenarios-100",
  baseline: {
    metrics: [
      { id: "throughput_week", value: 0.0, as_of: "2026-09-25T09:13:27.618335Z" },
      { id: "compliance.ai-act", value: null, as_of: "2026-09-25T09:13:27.618335Z", reason: 'no catalogue loaded for framework "ai-act"' },
      { id: "compliance.cra", value: 0.0, as_of: "2026-09-25T09:13:27.618335Z" },
    ],
    drivers: { capacity_factor: 1.0, rework_rate: 0.0, throughput_week: 0.0 },
    policy: [
      { framework: "cra", counts: { satisfied: 0, attested: 0, stale: 0, open: 5, not_applicable: 0 }, best_practice: { satisfied: 0, attested: 0, stale: 0, open: 0, not_applicable: 0 }, compliant: false },
    ],
    forecast: { per_week: [{ p10: 0, p50: 0, p90: 0 }, { p10: 0, p50: 0, p90: 0 }], completion_week: { p10: 0, p50: 0, p90: 0 }, samples: 1000, seed: 7575905717977004489 },
  },
  scenarios: [
    {
      scenario: {
        name: "ai-act-2027",
        title: "EU AI Act applies to our agents from 2027",
        kind: ["policy", "drivers"],
        assumptions: "High-risk classification for the customer-facing product.\n",
        horizon: "26w",
        from: "2026-09-01",
        policy: { add_frameworks: ["ai-act"], tighten: { "cra/annex-i-2-1": { max_age: "2w" } }, drop_not_applicable: [] },
        goals: [],
        drivers: { capacity_factor: "×0.8", first_pass_yield: "-10%" },
        signposts: [
          { metric: "compliance.ai-act", below: 0.5, from: "2027-01-01" },
          { metric: "throughput_week", below: 8.0 },
        ],
      },
      findings: [],
      policy: [
        {
          scope: "dev-scenarios-100",
          delta: {
            newly_open: ["ai-act/annex-iii-1", "ai-act/annex-iii-2"],
            newly_stale: [],
            newly_applicable_but_covered: [],
            unchanged: 5,
            missing_from_scenario: [],
            per_framework: [
              { framework: "ai-act", before: null, after: { framework: "ai-act", counts: { satisfied: 0, attested: 0, stale: 0, open: 2, not_applicable: 0 }, best_practice: { satisfied: 0, attested: 0, stale: 0, open: 0, not_applicable: 0 }, compliant: false } },
            ],
          },
        },
      ],
      policy_subtree: {
        newly_open: ["ai-act/annex-iii-1", "ai-act/annex-iii-2"],
        newly_stale: [],
        newly_applicable_but_covered: [],
        unchanged: 5,
        missing_from_scenario: [],
        per_framework: [
          { framework: "ai-act", before: null, after: { framework: "ai-act", counts: { satisfied: 0, attested: 0, stale: 0, open: 2, not_applicable: 0 }, best_practice: { satisfied: 0, attested: 0, stale: 0, open: 0, not_applicable: 0 }, compliant: false } },
        ],
      },
      drivers: {
        overridden: { capacity_factor: 0.8, rework_rate: 0.0, throughput_week: 0.0 },
        outcomes_before: { effective_throughput: 0.0 },
        outcomes_after: { effective_throughput: 0.0 },
        tornado: [
          { driver: "capacity_factor", low_outcome: 0.0, high_outcome: 0.0, span: 0.0 },
          { driver: "throughput_week", low_outcome: 0.0, high_outcome: 0.0, span: 0.0 },
        ],
      },
      backlog: { total: 2.0, newly_open_controls: 2, open_goal_tasks: 0 },
      forecast: { per_week: [{ p10: 0, p50: 0, p90: 0 }, { p10: 0, p50: 0, p90: 0 }], completion_week: { p10: null, p50: null, p90: null }, samples: 1000, seed: 878262586672636746 },
      goals: [],
      signposts: [
        { metric: "compliance.ai-act", state: "not_yet_active", reason: "active from 2027-01-01" },
        { metric: "throughput_week", state: "triggered", reason: "0 is below 8" },
      ],
    },
  ],
  findings: [],
  policy_findings: [],
  triggered: [{ scenario: "ai-act-2027", metric: "throughput_week", reason: "0 is below 8" }],
};

const REAL_METRICS_REGISTRY = [
  { id: "throughput_week", title: "Throughput per week", description: "Finished runs in the trailing 7 days.", unit: "per_week", better: "higher", source: "production.rs", available: true, unavailable_reason: null },
  { id: "compliance.ai-act", title: "Compliance share (ai-act)", description: "…", unit: "ratio", better: "higher", source: "policy::rollup", available: true, unavailable_reason: null },
  { id: "compliance.cra", title: "Compliance share (cra)", description: "…", unit: "ratio", better: "higher", source: "policy::rollup", available: true, unavailable_reason: null },
];

function stubElement() {
  return { innerHTML: "", textContent: "", hidden: false, querySelectorAll: () => [] };
}

test("loading asks for the selected scope, then renders the board, delta table, driver panel and matrix", async () => {
  const ids = [
    "scn-scope-note",
    "scn-error",
    "scn-count",
    "scn-signposts",
    "scn-board",
    "scn-delta",
    "scn-drivers",
    "scn-matrix",
    "scn-workshop",
    "scn-findings",
    "scn-driver-select",
    "scn-driver-reset",
    "scn-matrix-select",
    "scn-outcomes",
    "scn-tornado",
  ];
  const elements = {};
  for (const id of ids) elements[id] = stubElement();
  globalThis.document = { ...bare, getElementById: (id) => elements[id] || null };

  const requested = [];
  globalThis.fetch = async (path, opts) => {
    requested.push(path);
    if (path.startsWith("/api/scenarios/whatif")) {
      return {
        status: 200,
        statusText: "OK",
        json: async () => ({
          status: "ok",
          data: {
            kind: "scenario_what_if",
            result: {
              drivers: { overridden: { capacity_factor: 1.0, rework_rate: 0.0, throughput_week: 0.0 }, outcomes_before: { effective_throughput: 0.0 }, outcomes_after: { effective_throughput: 0.0 }, tornado: [] },
              forecast: { per_week: [], completion_week: { p10: null, p50: null, p90: null }, samples: 1000, seed: 1 },
            },
          },
        }),
      };
    }
    if (path.startsWith("/api/metrics")) {
      return { status: 200, statusText: "OK", json: async () => ({ status: "ok", data: { kind: "metrics", values: [], series: [], registry: REAL_METRICS_REGISTRY } }) };
    }
    return { status: 200, statusText: "OK", json: async () => ({ status: "ok", data: { kind: "scenarios", report: REAL_REPORT } }) };
  };
  state.scope = "dev-scenarios-100";

  await loadScenarios();

  assert.equal(requested[0], "/api/scenarios?scope=dev-scenarios-100");
  assert.ok(requested.some((p) => p.startsWith("/api/metrics?ids=")));
  assert.ok(requested.some((p) => p.startsWith("/api/scenarios/whatif")));

  assert.equal(elements["scn-count"].textContent, "1 scenario");
  assert.match(elements["scn-board"].innerHTML, /EU AI Act applies to our agents from 2027/);
  assert.match(elements["scn-board"].innerHTML, /Baseline/);
  assert.match(elements["scn-board"].innerHTML, /class="scn-fan"/, "a fan chart is drawn for both the baseline and the scenario card");
  assert.match(elements["scn-signposts"].innerHTML, /scn-sp-triggered/, "the triggered throughput signpost lights up");
  assert.match(elements["scn-delta"].innerHTML, /Effective throughput/);
  assert.match(elements["scn-drivers"].innerHTML, /Capacity factor/);
  // Not just the `<option>` text (which would pass on an empty table too):
  // the cell itself, clickable and carrying its own scope/framework data.
  assert.match(elements["scn-matrix"].innerHTML, /data-matrix-scope="dev-scenarios-100" data-matrix-fw="ai-act"/);
  assert.match(elements["scn-matrix"].innerHTML, /2 open/);
  assert.match(elements["scn-findings"].innerHTML, /No findings/);

  globalThis.fetch = async () => ({ status: 400, statusText: "Bad Request", json: async () => ({ status: "error", message: "no such scope: gone" }) });
  await loadScenarios();
  assert.equal(elements["scn-error"].textContent, "no such scope: gone");
  assert.equal(elements["scn-error"].hidden, false);
  assert.equal(elements["scn-board"].innerHTML, "", "a failed fetch clears the last answer rather than leaving it stale");

  delete globalThis.fetch;
  globalThis.document = bare;
  state.scope = null;
  renderScenarios(); // no throw with document stubbed back to the bare shim
});

test("a link with a level segment and an old bare link both land on the Scenarios page", () => {
  setRouter({ pages: ["dashboard", "goals", "policy", "scenarios"], redirects: {} });
  globalThis.location = { hash: "#all/dir/scenarios" };
  assert.equal(readHash().page, "scenarios");
  delete globalThis.location;
  globalThis.location = { hash: "#all/scenarios" };
  assert.equal(readHash().page, "scenarios");
  delete globalThis.location;
});
