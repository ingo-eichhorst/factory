import test from "node:test";
import assert from "node:assert/strict";

import {
  DRIVER_DEFS,
  THROUGHPUT_HISTORY_WEEKS,
  complianceSummary,
  completionSummary,
  controlReason,
  deltaTable,
  deltaTone,
  driverRange,
  driverUnavailableReason,
  fanGeometry,
  formatWeek,
  goalProbabilityLabel,
  krLabel,
  matrixCells,
  metricTitle,
  narrativeBoard,
  policyTabHref,
  promoteBody,
  signpostRows,
  signpostStrip,
  sliderToOverride,
  tornadoLayout,
  whatifBody,
} from "../js/scenarios-model.js";

// Trimmed (per_week cut to three points) versions of a real
// `GET /api/scenarios` answer, captured from a throwaway daemon on
// 127.0.0.1:8806 running `examples/policies/cra.yaml` + a draft
// `examples/policies/drafts/ai-act.yaml`, `examples/goals/`, and
// `examples/scenarios/{ai-act-2027,capacity-drop}.yaml` (the same two
// examples README.md's own load-with-zero-findings test pins), the root
// declaring `policies: {frameworks: [cra]}` -- not a shape invented by
// reading the Rust. A fresh instance with no finished runs makes every
// throughput-derived number zero and every forecast degenerate (`p10=p50=
// p90=0`, `completion_week: null` when a backlog exists); that is the
// honest answer for an empty instance, not a fixture bug.

const REAL_BASELINE = {
  metrics: [
    { id: "throughput_week", value: 0.0, as_of: "2026-09-25T09:13:27.618335Z" },
    { id: "first_pass_yield", value: null, as_of: "2026-09-25T09:13:27.618335Z", reason: "no finished runs in the trailing 28 days" },
    { id: "scrap_rate", value: null, as_of: "2026-09-25T09:13:27.618335Z", reason: "no finished runs in the trailing 28 days" },
    { id: "unit_cost", value: null, as_of: "2026-09-25T09:13:27.618335Z", reason: "a Run records no model, tokens or cost yet (design §12.6)" },
    { id: "tokens_per_run", value: null, as_of: "2026-09-25T09:13:27.618335Z", reason: "a Run records no model, tokens or cost yet (design §12.6)" },
    { id: "compliance.ai-act", value: null, as_of: "2026-09-25T09:13:27.618335Z", reason: 'no catalogue loaded for framework "ai-act"' },
    { id: "compliance.cra", value: 0.0, as_of: "2026-09-25T09:13:27.618335Z" },
  ],
  drivers: { capacity_factor: 1.0, rework_rate: 0.0, throughput_week: 0.0 },
  policy: [
    { framework: "cra", counts: { satisfied: 0, attested: 0, stale: 0, open: 5, not_applicable: 0 }, best_practice: { satisfied: 0, attested: 0, stale: 0, open: 0, not_applicable: 0 }, compliant: false },
  ],
  forecast: {
    per_week: [{ p10: 0, p50: 0, p90: 0 }, { p10: 0, p50: 0, p90: 0 }, { p10: 0, p50: 0, p90: 0 }],
    completion_week: { p10: 0, p50: 0, p90: 0 },
    samples: 1000,
    seed: 7575905717977004489,
  },
};

const REAL_AI_ACT_RESULT = {
  scenario: {
    name: "ai-act-2027",
    title: "EU AI Act applies to our agents from 2027",
    kind: ["policy", "drivers"],
    assumptions: "High-risk classification for the customer-facing product; human oversight\nrequired from the 2027 deadline.\n",
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
        newly_open: ["ai-act/annex-iii-1", "ai-act/annex-iii-2", "ai-act/annex-iii-3", "ai-act/annex-iii-4", "ai-act/annex-iii-5"],
        newly_stale: [],
        newly_applicable_but_covered: [],
        unchanged: 5,
        missing_from_scenario: [],
        per_framework: [
          { framework: "ai-act", before: null, after: { framework: "ai-act", counts: { satisfied: 0, attested: 0, stale: 0, open: 5, not_applicable: 0 }, best_practice: { satisfied: 0, attested: 0, stale: 0, open: 0, not_applicable: 0 }, compliant: false } },
          { framework: "cra", before: { framework: "cra", counts: { satisfied: 0, attested: 0, stale: 0, open: 5, not_applicable: 0 }, best_practice: { satisfied: 0, attested: 0, stale: 0, open: 0, not_applicable: 0 }, compliant: false }, after: { framework: "cra", counts: { satisfied: 0, attested: 0, stale: 0, open: 5, not_applicable: 0 }, best_practice: { satisfied: 0, attested: 0, stale: 0, open: 0, not_applicable: 0 }, compliant: false } },
        ],
      },
    },
  ],
  policy_subtree: {
    newly_open: ["ai-act/annex-iii-1", "ai-act/annex-iii-2", "ai-act/annex-iii-3", "ai-act/annex-iii-4", "ai-act/annex-iii-5"],
    newly_stale: [],
    newly_applicable_but_covered: [],
    unchanged: 5,
    missing_from_scenario: [],
    per_framework: [
      { framework: "ai-act", before: null, after: { framework: "ai-act", counts: { satisfied: 0, attested: 0, stale: 0, open: 5, not_applicable: 0 }, best_practice: { satisfied: 0, attested: 0, stale: 0, open: 0, not_applicable: 0 }, compliant: false } },
      { framework: "cra", before: { framework: "cra", counts: { satisfied: 0, attested: 0, stale: 0, open: 5, not_applicable: 0 }, best_practice: { satisfied: 0, attested: 0, stale: 0, open: 0, not_applicable: 0 }, compliant: false }, after: { framework: "cra", counts: { satisfied: 0, attested: 0, stale: 0, open: 5, not_applicable: 0 }, best_practice: { satisfied: 0, attested: 0, stale: 0, open: 0, not_applicable: 0 }, compliant: false } },
    ],
  },
  drivers: {
    overridden: { capacity_factor: 0.8, rework_rate: 0.0, throughput_week: 0.0 },
    outcomes_before: { effective_throughput: 0.0 },
    outcomes_after: { effective_throughput: 0.0 },
    tornado: [
      { driver: "capacity_factor", low_outcome: 0.0, high_outcome: 0.0, span: 0.0 },
      { driver: "rework_rate", low_outcome: 0.0, high_outcome: 0.0, span: 0.0 },
      { driver: "throughput_week", low_outcome: 0.0, high_outcome: 0.0, span: 0.0 },
    ],
  },
  backlog: { total: 5.0, newly_open_controls: 5, open_goal_tasks: 0 },
  forecast: {
    per_week: [{ p10: 0, p50: 0, p90: 0 }, { p10: 0, p50: 0, p90: 0 }, { p10: 0, p50: 0, p90: 0 }],
    completion_week: { p10: null, p50: null, p90: null },
    samples: 1000,
    seed: 878262586672636746,
  },
  goals: [],
  signposts: [
    { metric: "compliance.ai-act", state: "not_yet_active", reason: "active from 2027-01-01" },
    { metric: "throughput_week", state: "triggered", reason: "0 is below 8" },
  ],
};

const REAL_CAPACITY_DROP_RESULT = {
  scenario: {
    name: "capacity-drop",
    title: "Capacity drops 20% next quarter",
    kind: ["drivers", "goals"],
    assumptions: "A senior reviewer goes part-time; throughput and first-pass yield both dip\nuntil the backlog of in-flight reviews clears.\n",
    horizon: "13w",
    from: "2026-10-01",
    goals: [{ kr: "ship-compliant/cra-open-zero", by: "2027-03-31" }],
    drivers: { capacity_factor: "-20%", scrap_rate: "+5%" },
    signposts: [],
  },
  findings: [],
  policy: [
    {
      scope: "dev-scenarios-100",
      delta: {
        newly_open: [],
        newly_stale: [],
        newly_applicable_but_covered: [],
        unchanged: 5,
        missing_from_scenario: [],
        per_framework: [
          { framework: "cra", before: { framework: "cra", counts: { satisfied: 0, attested: 0, stale: 0, open: 5, not_applicable: 0 }, best_practice: { satisfied: 0, attested: 0, stale: 0, open: 0, not_applicable: 0 }, compliant: false }, after: { framework: "cra", counts: { satisfied: 0, attested: 0, stale: 0, open: 5, not_applicable: 0 }, best_practice: { satisfied: 0, attested: 0, stale: 0, open: 0, not_applicable: 0 }, compliant: false } },
        ],
      },
    },
  ],
  policy_subtree: {
    newly_open: [],
    newly_stale: [],
    newly_applicable_but_covered: [],
    unchanged: 5,
    missing_from_scenario: [],
    per_framework: [
      { framework: "cra", before: { framework: "cra", counts: { satisfied: 0, attested: 0, stale: 0, open: 5, not_applicable: 0 }, best_practice: { satisfied: 0, attested: 0, stale: 0, open: 0, not_applicable: 0 }, compliant: false }, after: { framework: "cra", counts: { satisfied: 0, attested: 0, stale: 0, open: 5, not_applicable: 0 }, best_practice: { satisfied: 0, attested: 0, stale: 0, open: 0, not_applicable: 0 }, compliant: false } },
    ],
  },
  drivers: {
    overridden: { capacity_factor: 0.8, rework_rate: 0.0, throughput_week: 0.0 },
    outcomes_before: { effective_throughput: 0.0 },
    outcomes_after: { effective_throughput: 0.0 },
    tornado: [
      { driver: "capacity_factor", low_outcome: 0.0, high_outcome: 0.0, span: 0.0 },
      { driver: "rework_rate", low_outcome: 0.0, high_outcome: 0.0, span: 0.0 },
      { driver: "throughput_week", low_outcome: 0.0, high_outcome: 0.0, span: 0.0 },
    ],
  },
  backlog: { total: 0.0, newly_open_controls: 0, open_goal_tasks: 0 },
  forecast: {
    per_week: [{ p10: 0, p50: 0, p90: 0 }, { p10: 0, p50: 0, p90: 0 }, { p10: 0, p50: 0, p90: 0 }],
    completion_week: { p10: 0, p50: 0, p90: 0 },
    samples: 1000,
    seed: 17719756888242083117,
  },
  goals: [
    {
      kr: "ship-compliant/cra-open-zero",
      target: 1.0,
      by: "2027-03-31",
      probability: { probability: null, reason: "ratio key results have no rate model to forecast a completion probability from; record a check-in, or use a driver what-if instead" },
    },
  ],
  signposts: [],
};

const REAL_REPORT = {
  scope: null,
  baseline: REAL_BASELINE,
  scenarios: [REAL_AI_ACT_RESULT, REAL_CAPACITY_DROP_RESULT],
  findings: [],
  policy_findings: [],
  triggered: [{ scenario: "ai-act-2027", metric: "throughput_week", reason: "0 is below 8" }],
};

// ------------------------------------------------------------------ drivers

test("DRIVER_DEFS mirrors the seven built-in drivers, cost drivers named but not assumptions", () => {
  const ids = DRIVER_DEFS.map((d) => d.id);
  assert.deepEqual(ids, ["throughput_week", "first_pass_yield", "scrap_rate", "rework_rate", "capacity_factor", "unit_cost", "tokens_per_run"]);
  // Backed by the registry metric of the same name since #106.
  assert.equal(DRIVER_DEFS.find((d) => d.id === "rework_rate").assumption, false);
  assert.equal(DRIVER_DEFS.find((d) => d.id === "rework_rate").metric, "rework_rate");
  assert.equal(DRIVER_DEFS.find((d) => d.id === "capacity_factor").assumption, true);
  // unit_cost/tokens_per_run *name* a registry metric -- measured since
  // #117 -- so they are not bare assumptions. #164 models their outcomes.
  assert.equal(DRIVER_DEFS.find((d) => d.id === "unit_cost").assumption, false);
  assert.equal(DRIVER_DEFS.find((d) => d.id === "unit_cost").metric, "unit_cost");
  assert.deepEqual(DRIVER_DEFS.filter((d) => d.measuredCost).map((d) => d.id), ["unit_cost", "tokens_per_run"]);
  assert.deepEqual(DRIVER_DEFS.filter((d) => d.unavailable), []);
});

test("driverRange: ratios 0..1, capacity_factor 0..2, throughput scales with the baseline", () => {
  assert.deepEqual(driverRange("first_pass_yield", 0), { min: 0, max: 1, step: 0.01 });
  assert.deepEqual(driverRange("capacity_factor", 1), { min: 0, max: 2, step: 0.05 });
  assert.deepEqual(driverRange("throughput_week", 0), { min: 0, max: 10, step: 0.5 });
  assert.deepEqual(driverRange("throughput_week", 40), { min: 0, max: 80, step: 0.5 });
  assert.deepEqual(driverRange("throughput_week", undefined), { min: 0, max: 10, step: 0.5 });
  assert.deepEqual(driverRange("unit_cost", 4), { min: 0, max: 8, step: 0.01 });
  assert.deepEqual(driverRange("tokens_per_run", 4000), { min: 0, max: 8000, step: 1 });
});

test("driverUnavailableReason: a def's own reason first, then baseline.metrics' reason, then the registry, then a fallback", () => {
  const cost = DRIVER_DEFS.find((d) => d.id === "unit_cost");
  // The cohort's own reason remains visible even if the registry fetch fails.
  const fromBaseline = [{ id: "unit_cost", value: null, reason: "no run finished in the trailing 28 days" }];
  assert.equal(driverUnavailableReason(cost, fromBaseline, {}), "no run finished in the trailing 28 days");
  const def = { id: "future", metric: "future_metric", unavailable: true };
  assert.equal(driverUnavailableReason(def, [{ id: "future_metric", value: null, reason: "not yet" }], {}), "not yet");
  assert.equal(driverUnavailableReason(def, [], { future_metric: { unavailable_reason: "registry says so" } }), "registry says so");
  assert.equal(driverUnavailableReason(def, [], {}), "a complete measured baseline is not available for this scope");
  assert.equal(driverUnavailableReason(def, null, null), "a complete measured baseline is not available for this scope");
});

test("whatifBody keeps a selected scope while leaving old unscoped requests compatible", () => {
  assert.deepEqual(whatifBody(null, { unit_cost: 2 }, "demo"), { drivers: { unit_cost: "=2" }, scope: "demo" });
  assert.deepEqual(whatifBody(null, {}), { drivers: {} });
});

// ------------------------------------------------------------------ metrics

test("metricTitle prefers a fetched registry def, then a fixed title, then a family-prefix guess", () => {
  assert.equal(metricTitle("throughput_week", {}), "Throughput per week");
  assert.equal(metricTitle("compliance.ai-act", {}), "Compliance share (ai-act)");
  assert.equal(metricTitle("open_controls.cra", {}), "Open controls (cra)");
  assert.equal(metricTitle("goal_tasks_done.ship-compliant.cra-open-zero", {}), "Goal tasks done (ship-compliant.cra-open-zero)");
  assert.equal(metricTitle("bench.resolve_rate.eval-set-a", {}), "Bench resolve rate (eval-set-a)");
  assert.equal(metricTitle("mystery.metric", {}), "mystery.metric");
  assert.equal(metricTitle("compliance.ai-act", { "compliance.ai-act": { title: "Compliance share (ai-act)" } }), "Compliance share (ai-act)");
  // A registry def with no title falls through to the guess, never `undefined`.
  assert.equal(metricTitle("compliance.ai-act", { "compliance.ai-act": {} }), "Compliance share (ai-act)");
});

// ---------------------------------------------------------------- signposts

test("signpostRows zips a scenario's own signpost definitions against its statuses by index, real shape", () => {
  const rows = signpostRows(REAL_AI_ACT_RESULT);
  assert.equal(rows.length, 2);
  assert.equal(rows[0].metric, "compliance.ai-act");
  assert.equal(rows[0].state, "not_yet_active");
  assert.equal(rows[0].cls, "scn-sp-not_yet_active");
  assert.equal(rows[0].label, "not yet active");
  assert.equal(rows[0].threshold, "below 0.5");
  assert.equal(rows[0].from, "2027-01-01");
  assert.equal(rows[1].metric, "throughput_week");
  assert.equal(rows[1].state, "triggered");
  assert.equal(rows[1].cls, "scn-sp-triggered");
  assert.equal(rows[1].reason, "0 is below 8");
  assert.equal(rows[1].threshold, "below 8");
});

test("signpostRows: a two-sided band reads 'outside above–below', and an above-only reads 'above N'", () => {
  const result = {
    scenario: { name: "x", title: "X", signposts: [{ metric: "m1", below: 0.4, above: 0.8 }, { metric: "m2", above: 0.9 }] },
    signposts: [{ metric: "m1", state: "quiet", reason: "within bounds" }, { metric: "m2", state: "quiet", reason: "within bounds" }],
  };
  const rows = signpostRows(result);
  assert.equal(rows[0].threshold, "outside 0.8–0.4");
  assert.equal(rows[1].threshold, "above 0.9");
});

test("signpostRows never reads past the shorter of the two arrays -- a mismatch is dropped, not thrown", () => {
  const result = { scenario: { name: "x", title: "X", signposts: [{ metric: "m1", below: 1 }] }, signposts: [] };
  assert.deepEqual(signpostRows(result), []);
});

test("signpostStrip flattens every scenario's own signposts, carrying which scenario each belongs to", () => {
  const rows = signpostStrip(REAL_REPORT);
  assert.equal(rows.length, 2); // ai-act-2027 has two; capacity-drop has none
  assert.ok(rows.every((r) => r.scenario === "ai-act-2027"));
});

// -------------------------------------------------------------- fan charts

test("fanGeometry: empty forecast (a reason set) is reported, not drawn", () => {
  const geo = fanGeometry({ per_week: [], completion_week: { p10: null, p50: null, p90: null }, reason: "no throughput history to forecast from" });
  assert.equal(geo.empty, true);
  assert.equal(geo.reason, "no throughput history to forecast from");
});

test("fanGeometry: a realistic widening band, origin at (0,height), p50 marker on the median line", () => {
  const forecast = {
    per_week: [
      { p10: 1, p50: 2, p90: 3 },
      { p10: 2, p50: 4, p90: 7 },
      { p10: 3, p50: 6, p90: 12 },
    ],
    completion_week: { p10: 3, p50: null, p90: 2 },
  };
  const geo = fanGeometry(forecast, { width: 300, height: 100 });
  assert.equal(geo.empty, false);
  assert.equal(geo.weeks, 3);
  // maxY is the largest p90 (12); week 0 sits at the bottom-left corner.
  const firstPoint = geo.bandPoints.split(" ")[0];
  assert.equal(firstPoint, "0.0,100.0");
  // Week 3's p10 (the band's last lower-edge point, y for value 3 of maxY 12)
  // is 3/4 of the way up: y = 100 - (3/12)*100 = 75.
  const points = geo.bandPoints.split(" ");
  assert.equal(points[3], "300.0,75.0");
  // No p50 marker (completion_week.p50 is null); a p90 marker at week 2, on
  // the p90 line (value 7 of maxY 12: y = 100 - (7/12)*100 = 41.7).
  assert.equal(geo.p50Marker, null);
  assert.ok(geo.p90Marker);
  assert.equal(geo.p90Marker.week, 2);
  // A marker's own {x, y} is raw, full-precision geometry (only the SVG
  // point *strings* -- `bandPoints`/`medianPoints` -- are rounded to one
  // decimal for a shorter `points=` attribute).
  assert.equal(geo.p90Marker.y, 100 - (7 / 12) * 100);
  // Axis scaffolding: yMax is the same 12 the band itself scales against;
  // xTicks is "now" (week 0), a midpoint (week 2, rounded from 3/2), and
  // the horizon's own last week (3) -- three ticks, never more.
  assert.equal(geo.yMax, 12);
  assert.deepEqual(geo.xTicks, [
    { x: 0, label: "now" },
    { x: 200, week: 2, label: "week 2" },
    { x: 300, week: 3, label: "week 3" },
  ]);
  assert.equal(geo.noSignal, false);
  assert.equal(geo.clearedAlready, false);
});

test("fanGeometry: xTicks skips the midpoint on a one- or two-week horizon rather than colliding with 'now' or the last week", () => {
  const oneWeek = fanGeometry({ per_week: [{ p10: 0, p50: 0, p90: 1 }], completion_week: { p10: null, p50: null, p90: null } });
  assert.deepEqual(
    oneWeek.xTicks.map((t) => t.label),
    ["now", "week 1"],
  );
  const twoWeek = fanGeometry({ per_week: [{ p10: 0, p50: 0, p90: 1 }, { p10: 0, p50: 0, p90: 1 }], completion_week: { p10: null, p50: null, p90: null } });
  assert.deepEqual(
    twoWeek.xTicks.map((t) => t.label),
    ["now", "week 1", "week 2"],
  );
});

test("fanGeometry: noSignal is true only when every week's own p90 is exactly zero", () => {
  const flat = { per_week: [{ p10: 0, p50: 0, p90: 0 }, { p10: 0, p50: 0, p90: 0 }], completion_week: { p10: null, p50: null, p90: null } };
  assert.equal(fanGeometry(flat, {}).noSignal, true);
  const oneNonZero = { per_week: [{ p10: 0, p50: 0, p90: 0 }, { p10: 0, p50: 1, p90: 2 }], completion_week: { p10: null, p50: null, p90: null } };
  assert.equal(fanGeometry(oneNonZero, {}).noSignal, false);
});

test("fanGeometry: clearedAlready is true exactly when completion_week.p50 is 0 -- forecast_completion's own proof that backlog <= 0", () => {
  // `forecast_completion` only ever sets `completed_at = Some(0)` when
  // `backlog <= 0.0`, for every sample identically -- a real backlog > 0
  // can complete no earlier than week 1. So p50 === 0 is unambiguous, and
  // this holds even when the week's own throughput was real (non-zero).
  const cleared = { per_week: [{ p10: 3, p50: 5, p90: 9 }], completion_week: { p10: 0, p50: 0, p90: 0 } };
  assert.equal(fanGeometry(cleared, {}).clearedAlready, true);
  const notCleared = { per_week: [{ p10: 0, p50: 0, p90: 0 }], completion_week: { p10: null, p50: null, p90: null } };
  assert.equal(fanGeometry(notCleared, {}).clearedAlready, false);
  const clearsLater = { per_week: [{ p10: 0, p50: 1, p90: 2 }], completion_week: { p10: null, p50: 1, p90: null } };
  assert.equal(fanGeometry(clearsLater, {}).clearedAlready, false);
});

test("fanGeometry: a completion week outside 1..weeks (0, or past the horizon) yields no marker", () => {
  const zero = { per_week: [{ p10: 0, p50: 1, p90: 2 }], completion_week: { p10: null, p50: 0, p90: null } };
  assert.equal(fanGeometry(zero, {}).p50Marker, null); // week 0 is "already done", not week 1
  const beyond = { per_week: [{ p10: 0, p50: 1, p90: 2 }], completion_week: { p10: null, p50: 5, p90: null } };
  assert.equal(fanGeometry(beyond, {}).p50Marker, null); // week 5 > weeks (1)
});

test("fanGeometry: a backlog line is drawn only when a backlog is given, at the backlog's own height", () => {
  const forecast = { per_week: [{ p10: 0, p50: 5, p90: 10 }], completion_week: { p10: null, p50: null, p90: null } };
  assert.equal(fanGeometry(forecast, {}).backlogY, null);
  const geo = fanGeometry(forecast, { height: 100, backlog: 5 });
  // maxY is max(backlog=5, p90=10) = 10; backlog's own y = 100 - (5/10)*100 = 50.
  assert.equal(geo.backlogY, 50);
});

test("formatWeek: null/undefined reads as never, otherwise 'week N'", () => {
  assert.equal(formatWeek(null), "never within the horizon");
  assert.equal(formatWeek(undefined), "never within the horizon");
  assert.equal(formatWeek(0), "week 0");
  assert.equal(formatWeek(7), "week 7");
});

test("THROUGHPUT_HISTORY_WEEKS mirrors the daemon's own compiled-in constant", () => {
  assert.equal(THROUGHPUT_HISTORY_WEEKS, 26);
});

test("completionSummary: a known backlog <= 0 reads 'nothing to clear', regardless of what completion_week itself says", () => {
  const fc = { p50: 3, p90: 5 }; // even a real-looking forecast
  assert.equal(completionSummary(fc, 0), "nothing in the backlog to clear");
  assert.equal(completionSummary(fc, -1), "nothing in the backlog to clear");
});

test("completionSummary: a known backlog > 0 reads the p50/p90 band, never 'week 0' by itself", () => {
  const fc = { p50: 3, p90: 8 };
  assert.equal(completionSummary(fc, 5), "p50 week 3 · p90 week 8");
});

test("completionSummary: no known backlog (the baseline card) falls back to completion_week.p50 === 0 as the only proof it has", () => {
  assert.equal(completionSummary({ p50: 0, p90: 0 }, undefined), "nothing in the backlog to clear");
  assert.equal(completionSummary({ p50: 0, p90: 0 }, null), "nothing in the backlog to clear");
  assert.equal(completionSummary({ p50: 4, p90: 9 }, undefined), "p50 week 4 · p90 week 9");
});

// -------------------------------------------------------------- delta tone

test("deltaTone: null/undefined/non-finite on either side is unknown, never guessed", () => {
  assert.deepEqual(deltaTone(null, 5, "higher"), { tone: "unknown", mag: 0 });
  assert.deepEqual(deltaTone(5, undefined, "higher"), { tone: "unknown", mag: 0 });
  assert.deepEqual(deltaTone(5, NaN, "higher"), { tone: "unknown", mag: 0 });
});

test("deltaTone: no change is flat regardless of direction", () => {
  assert.deepEqual(deltaTone(5, 5, "higher"), { tone: "flat", mag: 0 });
  assert.deepEqual(deltaTone(0, 0, "lower"), { tone: "flat", mag: 0 });
});

test("deltaTone: direction follows 'better', magnitude buckets on relative size", () => {
  // +1% -> mag 1 (rel < 0.05)
  assert.deepEqual(deltaTone(100, 101, "higher"), { tone: "good", mag: 1 });
  // -1% with better=higher -> bad, still mag 1
  assert.deepEqual(deltaTone(100, 99, "higher"), { tone: "bad", mag: 1 });
  // +10% -> mag 2 (0.05 <= rel < 0.2)
  assert.deepEqual(deltaTone(100, 110, "higher"), { tone: "good", mag: 2 });
  // +30% -> mag 3 (rel >= 0.2)
  assert.deepEqual(deltaTone(100, 130, "higher"), { tone: "good", mag: 3 });
  // better=lower flips the direction: a rise is bad.
  assert.deepEqual(deltaTone(100, 130, "lower"), { tone: "bad", mag: 3 });
  assert.deepEqual(deltaTone(100, 70, "lower"), { tone: "good", mag: 3 });
});

test("deltaTone: the mag 1/2 and 2/3 boundaries are exact (< not <=)", () => {
  assert.equal(deltaTone(100, 105, "higher").mag, 2); // exactly 5% -> not < 0.05, so mag 2
  assert.equal(deltaTone(100, 104.999, "higher").mag, 1);
  assert.equal(deltaTone(100, 120, "higher").mag, 3); // exactly 20% -> not < 0.2, so mag 3
  assert.equal(deltaTone(100, 119.999, "higher").mag, 2);
});

// -------------------------------------------------------------- delta table

test("deltaTable: drivers absent from both baseline and every scenario are omitted (unit_cost/tokens_per_run here)", () => {
  const t = deltaTable(REAL_REPORT);
  const keys = t.rows.map((r) => r.key);
  assert.ok(!keys.includes("driver:unit_cost"));
  assert.ok(!keys.includes("driver:tokens_per_run"));
  // first_pass_yield/scrap_rate: absent from REAL_BASELINE.drivers (no
  // finished runs to compute a baseline from) but ai-act-2027 sends its own
  // override anyway -- omitted here too, since `overridden` also lacks it
  // (`apply_overrides` silently drops a relative override with no baseline).
  assert.ok(!keys.includes("driver:first_pass_yield"));
  assert.ok(keys.includes("driver:capacity_factor"));
  assert.ok(keys.includes("driver:throughput_week"));
  assert.ok(keys.includes("outcome:effective_throughput"));
  assert.ok(keys.includes("forecast:p50"));
  assert.ok(keys.includes("forecast:p90"));
});

test("deltaTable: one row per framework either scenario's policy delta touches, union across scenarios", () => {
  const t = deltaTable(REAL_REPORT);
  const policyKeys = t.rows.filter((r) => r.kind === "policy").map((r) => r.key).sort();
  assert.deepEqual(policyKeys, ["policy:ai-act", "policy:cra"]);
  const aiAct = t.rows.find((r) => r.key === "policy:ai-act");
  // Both scenarios have a cell, in the same column order as `t.scenarios`;
  // capacity-drop never touches ai-act at all, so its `before`/`after` are
  // both null -- `unknown`, not zero.
  assert.equal(aiAct.cells[0].scenario, "ai-act-2027");
  assert.equal(aiAct.cells[0].before, null); // no `before` rollup: the framework did not exist yet
  assert.equal(aiAct.cells[0].after, 5); // open(5) + stale(0)
  assert.equal(aiAct.cells[0].tone, "unknown");
  assert.equal(aiAct.cells[1].scenario, "capacity-drop");
  assert.equal(aiAct.cells[1].before, null);
  assert.equal(aiAct.cells[1].after, null);
});

test("deltaTable: driver cells compare the shared baseline against each scenario's own overridden value", () => {
  const t = deltaTable(REAL_REPORT);
  const cap = t.rows.find((r) => r.key === "driver:capacity_factor");
  assert.equal(cap.cells[0].before, 1.0);
  assert.equal(cap.cells[0].after, 0.8); // ai-act-2027: ×0.8
  assert.equal(cap.cells[0].tone, "bad"); // capacity_factor's own `better` is "higher"
  assert.equal(cap.cells[1].after, 0.8); // capacity-drop: -20%, same resulting value
});

test("complianceSummary: counts newly open/stale and already-covered from the subtree delta, real shape", () => {
  assert.equal(complianceSummary(REAL_AI_ACT_RESULT), "5 newly open controls");
  assert.equal(complianceSummary(REAL_CAPACITY_DROP_RESULT), "No change to any control's status.");
});

test("complianceSummary: singular 'control', and covered is appended when present", () => {
  const one = { policy_subtree: { newly_open: ["cra/x"], newly_stale: [], newly_applicable_but_covered: [] } };
  assert.equal(complianceSummary(one), "1 newly open control");
  const covered = { policy_subtree: { newly_open: ["cra/x"], newly_stale: ["cra/y"], newly_applicable_but_covered: ["cra/z", "cra/w"] } };
  assert.equal(complianceSummary(covered), "1 newly open, 1 newly stale controls, 2 already covered");
});

// ---------------------------------------------------------------- tornado

test("tornadoLayout: sorts by span descending (server order), scales bars to a shared min/max", () => {
  const bars = [
    { driver: "a", low_outcome: 0, high_outcome: 10, span: 10 },
    { driver: "b", low_outcome: -5, high_outcome: 5, span: 10 },
    { driver: "c", low_outcome: 4, high_outcome: 6, span: 2 },
  ];
  const layout = tornadoLayout(bars, { width: 100 });
  // Ties on span 10 break by driver name ascending: "a" before "b".
  assert.deepEqual(layout.bars.map((b) => b.driver), ["a", "b", "c"]);
  assert.equal(layout.min, -5);
  assert.equal(layout.max, 10);
  const a = layout.bars[0];
  assert.equal(a.x, ((0 - -5) / 15) * 100);
  assert.equal(a.width, ((10 - -5) / 15) * 100 - a.x);
});

test("tornadoLayout: a zero-span bar is kept as a hairline, never dropped", () => {
  const bars = [
    { driver: "moves", low_outcome: 0, high_outcome: 10, span: 10 },
    { driver: "flat", low_outcome: 5, high_outcome: 5, span: 0 },
  ];
  const layout = tornadoLayout(bars, { width: 100 });
  const flat = layout.bars.find((b) => b.driver === "flat");
  assert.equal(flat.width, 0);
  assert.equal(flat.hairline, true);
});

test("tornadoLayout: an empty bar list is empty, not a division by zero", () => {
  assert.deepEqual(tornadoLayout([], { width: 100 }), { bars: [], min: 0, max: 0, width: 100 });
});

test("tornadoLayout: re-sorts an unsorted input the same way the server itself orders it", () => {
  const bars = [
    { driver: "z", low_outcome: 0, high_outcome: 1, span: 1 },
    { driver: "a", low_outcome: 0, high_outcome: 9, span: 9 },
  ];
  const layout = tornadoLayout(bars, {});
  assert.deepEqual(layout.bars.map((b) => b.driver), ["a", "z"]);
});

// ------------------------------------------------------------------ what-if

test("sliderToOverride: always Set (=N), rounded to four decimal places", () => {
  assert.equal(sliderToOverride(0.83), "=0.83");
  assert.equal(sliderToOverride(1), "=1");
  assert.equal(sliderToOverride(0.1 + 0.2), "=0.3"); // float noise rounds away
  assert.equal(sliderToOverride(0.123456789), "=0.1235");
});

test("whatifBody: only the drivers actually moved, each as a Set override; scenario omitted when null", () => {
  assert.deepEqual(whatifBody(null, { capacity_factor: 1.4 }), { drivers: { capacity_factor: "=1.4" } });
  assert.deepEqual(whatifBody("ai-act-2027", { capacity_factor: 1.4, rework_rate: 0.1 }), {
    scenario: "ai-act-2027",
    drivers: { capacity_factor: "=1.4", rework_rate: "=0.1" },
  });
  assert.deepEqual(whatifBody(null, {}), { drivers: {} });
});

test("promoteBody: agent omitted unless given", () => {
  assert.deepEqual(promoteBody("ai-act-2027", "demo"), { scenario: "ai-act-2027", scope: "demo" });
  assert.deepEqual(promoteBody("ai-act-2027", "demo", "claude-code"), { scenario: "ai-act-2027", scope: "demo", agent: "claude-code" });
});

// --------------------------------------------------------- policy what-if

test("matrixCells: buckets each scope's newly_open/newly_stale/covered controls by framework, real shape", () => {
  const m = matrixCells(REAL_AI_ACT_RESULT);
  assert.deepEqual(m.scopes, ["dev-scenarios-100"]);
  assert.deepEqual(m.frameworks, ["ai-act"]); // cra has no newly_open/stale/covered controls here
  const cell = m.cells["dev-scenarios-100"]["ai-act"];
  assert.equal(cell.newly_open.length, 5);
  assert.equal(cell.newly_stale.length, 0);
  assert.equal(cell.covered.length, 0);
});

test("matrixCells: a scenario with no policy overlay produces no scopes or frameworks", () => {
  const m = matrixCells({ policy: [] });
  assert.deepEqual(m, { scopes: [], frameworks: [], cells: {} });
});

test("controlReason: an add_frameworks control names the framework; a tightened one names its max_age; covered is its own reason", () => {
  assert.equal(controlReason(REAL_AI_ACT_RESULT.scenario, "ai-act/annex-iii-1", "newly_open"), "framework ai-act added by this scenario");
  assert.equal(controlReason(REAL_AI_ACT_RESULT.scenario, "cra/annex-i-2-1", "newly_stale"), "tightened to max_age 2w");
  assert.equal(controlReason(REAL_AI_ACT_RESULT.scenario, "cra/x", "covered"), "already satisfied or attested once this framework applies here");
});

test("controlReason: neither add_frameworks nor tighten names the control -- a generic overlay reason, not a crash", () => {
  const scenario = { policy: { add_frameworks: [], tighten: {}, drop_not_applicable: [] } };
  assert.equal(controlReason(scenario, "cra/x", "newly_open"), "newly open or stale under this scenario's overlay");
  assert.equal(controlReason({ policy: null }, "cra/x", "newly_open"), "newly open or stale under this scenario's overlay");
});

test("policyTabHref routes to the Policy tab for the given scope", () => {
  // No `/dir/` level segment: `routeHref` only inserts one once `initRail`
  // has handed it a `levelViews` map (`app.js`'s `LEVEL_VIEWS`), which this
  // isolated model test never calls -- the same restraint
  // `policy-model.test.js`'s own `refLinks` tests take (see its comment
  // above the same assertion).
  assert.equal(policyTabHref("demo"), "#demo/policy");
  assert.equal(policyTabHref(null), "#all/policy");
});

// --------------------------------------------------------------- workshop

test("narrativeBoard: groups narrative scenarios by their own quadrant name, first-encounter order", () => {
  const scenarios = [
    { scenario: { name: "tight", narrative: { axes: ["reg", "demand"], quadrant: "tightening-fast", drivers: ["d1"], premortem: ["p1"] } } },
    { scenario: { name: "calm", narrative: { axes: ["reg", "demand"], quadrant: "status-quo", drivers: [], premortem: [] } } },
    { scenario: { name: "also-tight", narrative: { quadrant: "tightening-fast", drivers: [], premortem: [] } } },
    { scenario: { name: "not-narrative" } },
  ];
  const board = narrativeBoard(scenarios);
  assert.deepEqual(board.axes, ["reg", "demand"]);
  assert.deepEqual(board.quadrants.map((q) => q.quadrant), ["tightening-fast", "status-quo"]);
  assert.equal(board.quadrants[0].scenarios.length, 2); // tight + also-tight share one quadrant
  assert.equal(board.pestle.length, 1); // only "tight" names any PESTLE drivers
  assert.equal(board.premortem.length, 1);
});

test("narrativeBoard: no narrative scenarios at all is an empty board, not a crash", () => {
  assert.deepEqual(narrativeBoard([]), { axes: null, quadrants: [], pestle: [], premortem: [] });
  assert.deepEqual(narrativeBoard([{ scenario: { name: "x" } }]).quadrants, []);
});

test("narrativeBoard: a scenario naming no quadrant still gets a card, grouped under a named placeholder", () => {
  const board = narrativeBoard([{ scenario: { name: "x", narrative: { drivers: [], premortem: [] } } }]);
  assert.deepEqual(board.quadrants.map((q) => q.quadrant), ["(no quadrant named)"]);
});

// -------------------------------------------------------------------- misc

test("krLabel: accepts the wire's plain 'objective/kr' string, and the {objective, kr} shape defensively", () => {
  assert.equal(krLabel("ship-compliant/cra-open-zero"), "ship-compliant/cra-open-zero");
  assert.equal(krLabel({ objective: "ship-compliant", kr: "cra-open-zero" }), "ship-compliant/cra-open-zero");
});

test("goalProbabilityLabel: a real None probability reads 'not modelled — <reason>', never a fabricated number", () => {
  const label = goalProbabilityLabel(REAL_CAPACITY_DROP_RESULT.goals[0].probability);
  assert.equal(label, "not modelled — ratio key results have no rate model to forecast a completion probability from; record a check-in, or use a driver what-if instead");
});

test("goalProbabilityLabel: a real probability rounds to a whole percent", () => {
  assert.equal(goalProbabilityLabel({ probability: 0.4, reason: null }), "40%");
  assert.equal(goalProbabilityLabel({ probability: 1, reason: "target already reached" }), "100%");
});
