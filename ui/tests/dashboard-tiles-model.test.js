import test from "node:test";
import assert from "node:assert/strict";

import { state } from "../js/core.js";
import { DEFAULT_DASHBOARD } from "../js/dashboard-model.js";
import {
  neededMetricIds,
  neededEndpoints,
  betterGlyph,
  coverageQualifier,
  metricTileView,
  agentHoursByScope,
  agentHoursByAgent,
  occupancyStripRows,
  complianceSummaryRows,
  topCostRows,
  windowDays,
} from "../js/dashboard-tiles-model.js";

// ------------------------------------------------------- neededMetricIds

test("neededMetricIds collects every metric tile's id, deduplicated and sorted", () => {
  const tiles = [
    { metric: "throughput_week", size: "s" },
    { view: "kpis", size: "xl" },
    { metric: "compliance.cra", size: "s" },
    { metric: "throughput_week", size: "s" },
  ];
  assert.deepEqual(neededMetricIds(tiles), ["compliance.cra", "throughput_week"]);
});

test("neededMetricIds is empty for a layout with no metric tile, DEFAULT_DASHBOARD included", () => {
  assert.deepEqual(neededMetricIds([]), []);
  assert.deepEqual(neededMetricIds(DEFAULT_DASHBOARD), []);
  assert.deepEqual(neededMetricIds([{ metric: "", size: "s" }, { metric: "   ", size: "s" }]), [], "a blank metric id is not a real id");
});

// -------------------------------------------------------- neededEndpoints

test("neededEndpoints reads false everywhere for DEFAULT_DASHBOARD -- the hard requirement that the default layout starts no new fetch", () => {
  assert.deepEqual(neededEndpoints(DEFAULT_DASHBOARD), {
    metrics: false,
    occupancy: false,
    operations: false,
    policy: false,
    costs: false,
  });
});

test("neededEndpoints flags exactly the endpoint each tile kind reads", () => {
  const only = (tile) => neededEndpoints([tile]);
  assert.deepEqual(only({ metric: "throughput_week", size: "s" }), { metrics: true, occupancy: false, operations: false, policy: false, costs: false });
  assert.deepEqual(only({ view: "agent_hours_by_scope", size: "m" }), { metrics: false, occupancy: true, operations: false, policy: false, costs: false });
  assert.deepEqual(only({ view: "agent_hours_by_agent", size: "m" }), { metrics: false, occupancy: true, operations: false, policy: false, costs: false });
  assert.deepEqual(only({ view: "occupancy_strip", size: "m" }), { metrics: false, occupancy: true, operations: false, policy: false, costs: false });
  assert.deepEqual(only({ view: "inbox", size: "m" }), { metrics: false, occupancy: false, operations: true, policy: false, costs: false });
  assert.deepEqual(only({ view: "compliance", size: "m" }), { metrics: false, occupancy: false, operations: false, policy: true, costs: false });
  assert.deepEqual(only({ view: "cost", size: "m" }), { metrics: false, occupancy: false, operations: false, policy: false, costs: true });
  // The five #163 views read state/production already in hand, nothing new.
  for (const view of ["kpis", "throughput", "on_the_line", "production_year", "by_scope"]) {
    assert.deepEqual(only({ view, size: "s" }), { metrics: false, occupancy: false, operations: false, policy: false, costs: false }, view);
  }
});

test("neededEndpoints combines flags across a mixed layout, each read at most once conceptually (one flag, however many tiles ask for it)", () => {
  const tiles = [
    { view: "agent_hours_by_scope", size: "m" },
    { view: "occupancy_strip", size: "m" },
    { metric: "unit_cost", size: "s" },
    { view: "cost", size: "m" },
  ];
  assert.deepEqual(neededEndpoints(tiles), { metrics: true, occupancy: true, operations: false, policy: false, costs: true });
});

// ------------------------------------------------------------ metric tiles

test("betterGlyph reads higher/lower, and anything else as no opinion", () => {
  assert.equal(betterGlyph("higher"), "↑");
  assert.equal(betterGlyph("lower"), "↓");
  assert.equal(betterGlyph(undefined), "");
  assert.equal(betterGlyph("sideways"), "");
});

test("coverageQualifier names instance-wide, and reads a missing/unknown field as nothing to add (#182 has not merged)", () => {
  assert.equal(coverageQualifier("instance_wide"), "instance-wide");
  assert.equal(coverageQualifier("scope_aware"), "");
  assert.equal(coverageQualifier(undefined), "", "a build against pre-#182 main sends no coverage field at all");
  assert.equal(coverageQualifier(null), "");
});

const METRICS_ANSWER = {
  values: [
    { id: "throughput_week", value: 6, as_of: "2026-09-26T00:00:00Z", reason: null },
    { id: "unit_cost", value: null, as_of: "2026-09-26T00:00:00Z", reason: "none of the 1 runs this trailing 28 days measured a cost" },
    { id: "bench.resolve_rate.eval-set-a", value: 0.5, as_of: "2026-09-20T00:00:00Z", reason: null },
  ],
  series: [
    { id: "throughput_week", points: [["2026-09-24", 4], ["2026-09-25", 5], ["2026-09-26", 6]] },
  ],
  registry: [
    { id: "throughput_week", title: "Throughput per week", unit: "per_week", better: "higher", coverage: "scope_aware", source: "s", available: true, unavailable_reason: null },
    { id: "unit_cost", title: "Unit cost", unit: "usd", better: "lower", coverage: "scope_aware", source: "s", available: true, unavailable_reason: null },
    { id: "bench.resolve_rate.eval-set-a", title: "Bench resolve rate (eval-set-a)", unit: "ratio", better: "higher", coverage: "instance_wide", source: "s", available: true, unavailable_reason: null },
  ],
};

test("metricTileView reads a real value, its unit, better-direction glyph, and its series as plain numbers oldest first", () => {
  const view = metricTileView("throughput_week", METRICS_ANSWER);
  assert.equal(view.title, "Throughput per week");
  assert.equal(view.unavailable, false);
  assert.equal(view.reason, null);
  assert.equal(view.valueText, "6.0/wk");
  assert.equal(view.better, "↑");
  assert.equal(view.coverage, "", "scope_aware adds no qualifier");
  assert.deepEqual(view.points, [4, 5, 6]);
});

test("metricTileView shows the wire's own reason for a value the daemon could not compute, never a manufactured number", () => {
  const view = metricTileView("unit_cost", METRICS_ANSWER);
  assert.equal(view.unavailable, true);
  assert.match(view.reason, /not measured yet/);
  assert.equal(view.valueText, null);
});

test("metricTileView names an instance-wide family, and reads a missing coverage field (pre-#182) as nothing to add", () => {
  const view = metricTileView("bench.resolve_rate.eval-set-a", METRICS_ANSWER);
  assert.equal(view.coverage, "instance-wide");
  const noCoverageField = { ...METRICS_ANSWER, registry: METRICS_ANSWER.registry.map(({ coverage, ...rest }) => rest) };
  assert.equal(metricTileView("bench.resolve_rate.eval-set-a", noCoverageField).coverage, "");
});

test("metricTileView never draws a sparkline for an all-zero series -- a flat zero reads as a stable trend, not as 'nothing has happened here yet'", () => {
  const allZero = {
    values: [{ id: "throughput_week", value: 0, as_of: "2026-09-26T00:00:00Z", reason: null }],
    series: [{ id: "throughput_week", points: [["2026-09-24", 0], ["2026-09-25", 0], ["2026-09-26", 0]] }],
    registry: [{ id: "throughput_week", title: "Throughput per week", unit: "per_week", better: "higher", source: "s", available: true, unavailable_reason: null }],
  };
  assert.equal(metricTileView("throughput_week", allZero).points, null);
  // A series with at least one real value still draws.
  const someReal = { ...allZero, series: [{ id: "throughput_week", points: [["2026-09-24", 0], ["2026-09-25", 2], ["2026-09-26", 0]] }] };
  assert.deepEqual(metricTileView("throughput_week", someReal).points, [0, 2, 0]);
});

test("metricTileView reads an id missing from the answer, or no answer at all, as 'not available right now' rather than throwing", () => {
  assert.equal(metricTileView("scrap_rate", METRICS_ANSWER).unavailable, true);
  assert.equal(metricTileView("scrap_rate", METRICS_ANSWER).reason, "not available right now");
  assert.equal(metricTileView("throughput_week", null).unavailable, true);
  assert.equal(metricTileView("throughput_week", undefined).unavailable, true);
  assert.equal(metricTileView("throughput_week", null).title, "throughput_week", "falls back to the bare id with no registry entry");
});

// -------------------------------------------------------------- agent hours

const OCC = {
  now: "2026-09-26T00:00:00Z",
  from: "2026-09-12T00:00:00Z",
  to: "2026-09-26T00:00:00Z",
  scopes: [
    {
      name: "root",
      path: "root",
      rows: [
        { agent: "alice", busy_seconds: 3600 * 5, blocked_seconds: 3600 },
        { agent: "bob", busy_seconds: 3600 * 2, blocked_seconds: 0 },
      ],
    },
    {
      name: "child",
      path: "root/child",
      rows: [{ agent: "carol", busy_seconds: 3600 * 9, blocked_seconds: 3600 * 0.5 }],
    },
  ],
};

test("agentHoursByScope sums busy/blocked seconds per scope, in hours, busiest first", () => {
  state.scope = null;
  state.scopes = [{ name: "root", path: "root" }, { name: "child", path: "root/child" }];
  const rows = agentHoursByScope(OCC);
  assert.deepEqual(rows.map((r) => r.label), ["child", "root"], "child's 9h beats root's 7h combined");
  const root = rows.find((r) => r.label === "root");
  assert.equal(root.busyHours, 7, "5h + 2h");
  assert.equal(root.blockedHours, 1, "never subtracted from busyHours above");
});

test("agentHoursByScope narrows to the selected scope's own subtree, the same rule every other tile follows", () => {
  state.scope = "child";
  state.scopes = [{ name: "root", path: "root" }, { name: "child", path: "root/child" }];
  const rows = agentHoursByScope(OCC);
  assert.deepEqual(rows.map((r) => r.label), ["child"]);
});

test("agentHoursByAgent keys by scope/agent and never confuses same-named agents across scopes", () => {
  state.scope = null;
  state.scopes = [{ name: "root", path: "root" }, { name: "child", path: "root/child" }];
  const rows = agentHoursByAgent(OCC);
  assert.deepEqual(rows.map((r) => r.key), ["child/carol", "root/alice", "root/bob"]);
  assert.equal(rows[0].busyHours, 9);
  assert.equal(rows.find((r) => r.key === "root/alice").blockedHours, 1);
});

test("agentHoursByScope/agentHoursByAgent read a missing or malformed occupancy answer as no rows, not a throw", () => {
  assert.deepEqual(agentHoursByScope(null), []);
  assert.deepEqual(agentHoursByScope(undefined), []);
  assert.deepEqual(agentHoursByScope({}), []);
  assert.deepEqual(agentHoursByAgent(null), []);
});

test("occupancyStripRows reads each agent's share of the answer's own window, clamped to 100%, busiest first", () => {
  state.scope = null;
  state.scopes = [{ name: "root", path: "root" }, { name: "child", path: "root/child" }];
  const nowMs = Date.parse("2026-09-26T00:00:00Z");
  const rows = occupancyStripRows(OCC, nowMs);
  // 14-day window = 1,209,600s; carol busy 32,400s => ~2.7%.
  const carol = rows.find((r) => r.agent === "carol");
  assert.equal(carol.pct, Math.round((3600 * 9 / (14 * 86400)) * 100));
  assert.ok(rows[0].pct >= rows[rows.length - 1].pct, "busiest first");
});

test("occupancyStripRows clips the window to now, so an answer whose `to` is in the future never divides by more than has actually elapsed", () => {
  const nowMs = Date.parse("2026-09-19T00:00:00Z"); // exactly the halfway point of OCC's 14-day window
  const rows = occupancyStripRows(OCC, nowMs);
  const alice = rows.find((r) => r.agent === "alice");
  // 5h busy over 7 elapsed days (604800s) rather than the full 14.
  assert.equal(alice.pct, Math.round((3600 * 5 / (7 * 86400)) * 100));
});

// ------------------------------------------------------------- compliance

const POLICY_REPORT = {
  catalogues: [{ framework: "cra", title: "Cyber Resilience Act", kind: "regulation" }],
  rollup: [
    {
      framework: "cra",
      compliant: false,
      counts: { satisfied: 1, attested: 1, stale: 0, open: 2, not_applicable: 1 },
      best_practice: { satisfied: 0, attested: 0, stale: 0, open: 0, not_applicable: 0 },
    },
  ],
};

test("complianceSummaryRows splits met (satisfied+attested+n/a) from open (open+stale), the same split the registry metrics use server-side", () => {
  const rows = complianceSummaryRows(POLICY_REPORT);
  assert.equal(rows.length, 1);
  assert.equal(rows[0].title, "Cyber Resilience Act");
  assert.equal(rows[0].met, 3);
  assert.equal(rows[0].open, 2);
  assert.equal(rows[0].counted, 5);
  assert.equal(rows[0].compliant, false);
});

test("complianceSummaryRows is empty for no report or no loaded framework", () => {
  assert.deepEqual(complianceSummaryRows(null), []);
  assert.deepEqual(complianceSummaryRows({ catalogues: [], rollup: [] }), []);
});

// -------------------------------------------------------------------- cost

const COST_REPORT = {
  group_by: "scope",
  rows: [
    { key: "root", label: "root", runs: 3, cost_usd: 9.5 },
    { key: "child", label: "child", runs: 1, cost_usd: 2.25 },
    { key: "other", label: "other", runs: 1, cost_usd: 0.1 },
  ],
  total: { runs: 5, cost_usd: 11.85 },
};

test("topCostRows truncates the wire's own most-expensive-first order without reordering", () => {
  assert.deepEqual(topCostRows(COST_REPORT, 2), COST_REPORT.rows.slice(0, 2));
  assert.deepEqual(topCostRows(COST_REPORT, 5), COST_REPORT.rows);
  assert.deepEqual(topCostRows(COST_REPORT), COST_REPORT.rows.slice(0, 5), "defaults to 5");
});

test("topCostRows is empty for no report", () => {
  assert.deepEqual(topCostRows(null), []);
  assert.deepEqual(topCostRows(undefined), []);
});

// ---------------------------------------------------------------- windowDays

test("windowDays rounds the whole days between two RFC 3339 timestamps -- what an answer's own from/to actually covers, not what was asked for", () => {
  assert.equal(windowDays("2026-09-01T00:00:00Z", "2026-10-01T00:00:00Z"), 30);
  assert.equal(windowDays("2026-09-26T00:00:00Z", "2026-09-26T00:00:00Z"), 0);
  // `/api/occupancy` clamps at 30 days regardless of what was asked (a d90
  // selection) -- this is exactly what lets a tile's own qualifier say the
  // truth instead of parroting the dashboard's window key.
  assert.equal(windowDays("2026-08-27T00:00:00Z", "2026-09-26T00:00:00Z"), 30);
});

test("windowDays is null for anything that does not parse, so a caller falls back to no qualifier rather than 'NaN days'", () => {
  assert.equal(windowDays(undefined, "2026-09-26T00:00:00Z"), null);
  assert.equal(windowDays("2026-09-26T00:00:00Z", "not a date"), null);
  assert.equal(windowDays(null, null), null);
});
