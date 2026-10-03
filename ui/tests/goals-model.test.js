import test from "node:test";
import assert from "node:assert/strict";

import {
  KR_RING_RADIUS,
  ORBIT_MAX_KRS,
  PRODUCTION_METRIC_IDS,
  alignsToEdges,
  bandBadgeClass,
  bandLabel,
  checkinBody,
  connectorPath,
  cycleOptionLabel,
  cycleProgress,
  danglingAlignsTo,
  defaultCycleId,
  findingLabel,
  findingsByKind,
  formatUnitValue,
  hasDirection,
  krHistoryValues,
  krScoredCount,
  krSourceLink,
  metricIdsForReport,
  metricUnavailableNote,
  normalizeViewMode,
  objectiveColorVar,
  objectiveLayers,
  orbitLayout,
  registryIndex,
  ringDashArray,
  ringGeometry,
  roadmapLanes,
  seriesIndex,
  sparklinePointsAttr,
  tableRows,
} from "../js/goals-model.js";

// A real `GET /api/goals` answer's `.report` (after `api()` unwraps
// `{status, data}`), captured from a throwaway daemon (root bound to
// 127.0.0.1:8803) running `examples/goals/direction.yaml` plus an added
// current-dated `2026-q3.yaml` cycle (today is 2026-09-25), `examples/policies/cra.yaml`
// declared at the root, three shell tasks dispatched in a `projects` scope
// (one labelled `goal=raise-quality/sbom-pipeline-done`), and two check-ins
// posted against the manual key result `ship-compliant/dpa-signed` -- not a
// shape invented by reading the Rust.
const report = {
  direction: {
    vision: "Every product this company ships can prove, on demand, that it is compliant and that the work behind it was done well.",
    mission: "Run agents that build and operate real products, and let the operating data those agents produce carry the evidence.",
    values: ["Evidence over assertion", "Compliance is closed at the level that causes it, never claimed at the top"],
    north_star: { metric: "first_pass_yield", why: "A run that needed no rework is the clearest sign the whole pipeline is working." },
    inputs: ["throughput_week", "first_pass_yield", "scrap_rate", "compliance.cra"],
    obstacles: ["Unit cost and tokens-per-run stay invisible until a Run records what it spent (design §12.6)."],
  },
  cycles: [
    { id: "2026-q2", from: "2026-04-01", to: "2026-06-30", status: "past", score: 1.0 },
    { id: "2026-q3", from: "2026-07-01", to: "2026-09-30", status: "current", score: 0.47916666666666663 },
    { id: "2026-q4", from: "2026-10-01", to: "2026-12-31", status: "future", score: 0.47916666666666663 },
  ],
  report: {
    cycle_id: "2026-q3",
    status: "current",
    elapsed: 0.9386102304750402,
    objectives: [
      {
        objective: "ship-compliant",
        title: "Every product can ship CRA-compliant",
        scope: "projects",
        score: 0.2916666666666667,
        key_results: [
          {
            kr: "ship-compliant/cra-open-zero",
            title: "No open CRA controls, instance-wide",
            kind: "committed",
            manual: false,
            metric: "compliance.cra",
            baseline: 0.0,
            target: 1.0,
            value: 0.0,
            score: 0.0,
            band: "red",
            on_pace: false,
            source: "metric compliance.cra (as of 2026-09-25)",
            reasons: [],
            confidence: null,
          },
          {
            kr: "ship-compliant/dpa-signed",
            title: "All processor agreements signed",
            kind: "aspirational",
            manual: true,
            baseline: 0.0,
            target: 12.0,
            value: 7.0,
            score: 0.5833333333333334,
            band: "yellow",
            on_pace: null,
            source: "check-in by owner on 2026-09-25",
            reasons: [],
            confidence: 8,
          },
        ],
      },
      {
        objective: "raise-quality",
        title: "Raise first-pass quality without slowing down",
        aligns_to: "ship-compliant",
        score: 0.6666666666666666,
        key_results: [
          {
            kr: "raise-quality/fpy-90",
            title: "First-pass yield at or above 90%",
            kind: "committed",
            manual: false,
            metric: "first_pass_yield",
            baseline: 0.5,
            target: 0.9,
            value: 1.0,
            score: 1.0,
            band: "green",
            on_pace: true,
            source: "metric first_pass_yield (as of 2026-09-25)",
            reasons: [],
            confidence: null,
          },
          {
            kr: "raise-quality/scrap-below-5",
            title: "Scrap rate below 5%",
            kind: "aspirational",
            manual: false,
            metric: "scrap_rate",
            baseline: 0.2,
            target: 0.05,
            value: 0.4,
            score: 0.0,
            band: "red",
            on_pace: null,
            source: "metric scrap_rate (as of 2026-09-25)",
            reasons: [],
            confidence: null,
          },
          {
            kr: "raise-quality/sbom-pipeline-done",
            title: "SBOM pipeline ships and stays green",
            kind: "committed",
            manual: false,
            metric: "goal_tasks_done.raise-quality.sbom-pipeline-done",
            baseline: 0.0,
            target: 1.0,
            value: 1.0,
            score: 1.0,
            band: "green",
            on_pace: true,
            source: "metric goal_tasks_done.raise-quality.sbom-pipeline-done (as of 2026-09-25)",
            reasons: [],
            confidence: null,
          },
        ],
      },
    ],
  },
  findings: [{ kind: "orphan_roadmap_item", subject: "2026-q3.yaml", detail: 'roadmap item "mystery-initiative" links no objective' }],
  north_star: {
    metric: "first_pass_yield",
    why: "A run that needed no rework is the clearest sign the whole pipeline is working.",
    value: { id: "first_pass_yield", value: 1.0, as_of: "2026-09-25T08:27:05.812819Z" },
  },
  inputs: [
    { metric: "throughput_week", value: { id: "throughput_week", value: 5.0, as_of: "2026-09-25T08:27:05.812819Z" } },
    { metric: "first_pass_yield", value: { id: "first_pass_yield", value: 1.0, as_of: "2026-09-25T08:27:05.812819Z" } },
    { metric: "scrap_rate", value: { id: "scrap_rate", value: 0.4, as_of: "2026-09-25T08:27:05.812819Z" } },
    { metric: "compliance.cra", value: { id: "compliance.cra", value: 0.0, as_of: "2026-09-25T08:27:05.812819Z" } },
  ],
  roadmap: [
    {
      id: "sbom-pipeline",
      title: "SBOM export pipeline",
      lane: "now",
      objectives: ["ship-compliant", "raise-quality"],
      why: "Closes the CRA's Annex I SBOM control.",
      scope: "projects",
    },
    { id: "dpa-tracker", title: "Processor agreement tracker", lane: "next", objectives: ["ship-compliant"], why: "A knowledge page per processor." },
    { id: "cost-visibility", title: "Per-run cost and token accounting", lane: "later", objectives: ["raise-quality"], why: "Blocked on design §12.6." },
    { id: "mystery-initiative", title: "Something somebody wrote down and never linked", lane: "now", objectives: [], why: "Deliberately an orphan." },
  ],
  checkins: {
    "ship-compliant/dpa-signed": [
      { id: "573ca6a2", kr: "ship-compliant/dpa-signed", value: 3.0, confidence: 6, note: "three DPAs signed so far", by: "owner", at: "2026-09-25T08:27:05.756243Z" },
      { id: "984d2418", kr: "ship-compliant/dpa-signed", value: 7.0, confidence: 8, by: "owner", at: "2026-09-25T08:27:05.784168Z" },
    ],
  },
};

// A real, trimmed `GET /api/metrics?ids=...` answer's `.data` from the same
// daemon (series trimmed to their first two and last two points).
const metricsAnswer = {
  kind: "metrics",
  values: [
    { id: "throughput_week", value: 5.0, as_of: "2026-09-25T08:26:58.107326Z" },
    { id: "first_pass_yield", value: 1.0, as_of: "2026-09-25T08:26:58.107326Z" },
    { id: "scrap_rate", value: 0.4, as_of: "2026-09-25T08:26:58.107326Z" },
    { id: "compliance.cra", value: 0.0, as_of: "2026-09-25T08:26:58.107326Z" },
    { id: "unit_cost", value: null, as_of: "2026-09-25T08:26:58.107326Z", reason: "a Run records no model, tokens or cost yet (design §12.6)" },
    { id: "goal_tasks_done.raise-quality.sbom-pipeline-done", value: 1.0, as_of: "2026-09-25T08:26:58.107326Z" },
  ],
  series: [
    {
      id: "throughput_week",
      points: [
        ["2025-09-20", 0.0],
        ["2025-09-21", 0.0],
        ["2026-09-24", 0.0],
        ["2026-09-25", 5.0],
      ],
    },
    {
      id: "first_pass_yield",
      points: [
        ["2026-09-25", 1.0],
        ["2026-09-25", 1.0],
      ],
    },
  ],
  registry: [
    { id: "throughput_week", title: "Throughput per week", description: "…", unit: "per_week", better: "higher", source: "…", available: true, unavailable_reason: null },
    { id: "first_pass_yield", title: "First-pass yield", description: "…", unit: "ratio", better: "higher", source: "…", available: true, unavailable_reason: null },
    { id: "scrap_rate", title: "Scrap rate", description: "…", unit: "ratio", better: "lower", source: "…", available: true, unavailable_reason: null },
    { id: "compliance.cra", title: "Compliance share (cra)", description: "…", unit: "ratio", better: "higher", source: "…", available: true, unavailable_reason: null },
    { id: "unit_cost", title: "Unit cost", description: "Cost per finished unit.", unit: "ratio", better: "lower", source: "none -- see unavailable_reason", available: false, unavailable_reason: "a Run records no model, tokens or cost yet (design §12.6)" },
  ],
};

// -------------------------------------------------------------- cycles

test("defaultCycleId prefers current, then the soonest future, then the most recent past", () => {
  assert.equal(defaultCycleId(report.cycles), "2026-q3");
  assert.equal(
    defaultCycleId(report.cycles.filter((c) => c.status !== "current")),
    "2026-q4",
    "no current cycle: the soonest future one",
  );
  assert.equal(
    defaultCycleId(report.cycles.filter((c) => c.status === "past")),
    "2026-q2",
    "only past cycles: the most recent one",
  );
  assert.equal(defaultCycleId([]), null);
  assert.equal(defaultCycleId(null), null);
});

test("cycleOptionLabel names id, status and a rounded percent score, or unscored", () => {
  assert.equal(cycleOptionLabel({ id: "2026-q3", status: "current", score: 0.479 }), "2026-q3 — current — 48% score");
  assert.equal(cycleOptionLabel({ id: "2026-q5", status: "future", score: null }), "2026-q5 — future — unscored");
});

test("cycleProgress reads elapsed as a percent, and scores as the mean of every objective's own score -- the same formula and the same field the cycle switcher's own CycleSummary.score already carries, so the two can never disagree", () => {
  const progress = cycleProgress(report.report);
  assert.equal(progress.elapsedPct, 93.9);
  // Objective scores: 0.2916666666666667 (ship-compliant), 0.6666666666666666
  // (raise-quality) -- mean 0.47916666666666663, exactly `report.cycles`'
  // own "2026-q3" entry in the real fixture this file's header comment
  // describes, confirming this reads the identical number the switcher does.
  assert.equal(progress.scorePct, 47.9);
});

test("cycleProgress is null with no report", () => {
  assert.equal(cycleProgress(null), null);
});

test("cycleProgress reads null scorePct when every objective is unscored", () => {
  const cr = { elapsed: 0, objectives: [{ score: null, key_results: [{ score: null }, { score: null }] }] };
  assert.deepEqual(cycleProgress(cr), { elapsedPct: 0, scorePct: null });
});

test("krScoredCount counts scored key results against every key result, independent of cycleProgress's own aggregate", () => {
  // Every key result in the fixture carries a score (the manual one has a
  // check-in), so this is 5 of 5; a fixture with an unscored manual key
  // result (no check-in yet) is covered by the empty-objective case below.
  assert.deepEqual(krScoredCount(report.report), { scored: 5, total: 5 });
  const partial = { objectives: [{ score: null, key_results: [{ score: 0.5 }, { score: null }] }] };
  assert.deepEqual(krScoredCount(partial), { scored: 1, total: 2 });
  assert.deepEqual(krScoredCount(null), { scored: 0, total: 0 });
});

// ---------------------------------------------------------------- bands

test("bandLabel and bandBadgeClass read null as unscored", () => {
  assert.equal(bandLabel("green"), "green");
  assert.equal(bandLabel(null), "unscored");
  assert.equal(bandBadgeClass("red"), "s-red");
  assert.equal(bandBadgeClass(null), "s-unscored");
});

// ----------------------------------------------------------------- rings

test("ringGeometry draws a full ring at score 1, empty at score null, and never negative or over-full", () => {
  const full = ringGeometry(1);
  assert.ok(full.offset < 0.01, "a full ring's offset is ~0");
  const empty = ringGeometry(null);
  assert.equal(empty.offset, empty.circumference, "an unscored key result draws a fully empty ring, not a manufactured zero");
  const half = ringGeometry(0.5);
  assert.ok(Math.abs(half.offset - half.circumference / 2) < 0.01);
  const clampedHigh = ringGeometry(1.4);
  assert.equal(clampedHigh.offset, 0);
  const clampedLow = ringGeometry(-0.2);
  assert.equal(clampedLow.offset, clampedLow.circumference);
  assert.equal(ringGeometry(0.5).radius, KR_RING_RADIUS);
});

test("ringDashArray is solid (null) for committed and dashed for aspirational", () => {
  const c = ringGeometry(1).circumference;
  assert.equal(ringDashArray("committed", c), null);
  assert.ok(ringDashArray("aspirational", c));
});

// -------------------------------------------------------------- sparklines

test("sparklinePointsAttr is null under two points, else a polyline points string", () => {
  assert.equal(sparklinePointsAttr([]), null);
  assert.equal(sparklinePointsAttr([1]), null);
  const pts = sparklinePointsAttr([1, 2, 3], 100, 26);
  assert.equal(pts.split(" ").length, 3);
  assert.match(pts, /^0\.0,26\.0 50\.0,13\.0 100\.0,0\.0$/);
});

test("krHistoryValues reads a manual key result's own check-in values, oldest first", () => {
  const kr = report.report.objectives[0].key_results[1]; // dpa-signed, manual
  assert.deepEqual(krHistoryValues(kr, report.checkins, {}), [3.0, 7.0]);
});

test("krHistoryValues is null for a manual key result with fewer than two check-ins", () => {
  const kr = { manual: true, kr: "no/history" };
  assert.equal(krHistoryValues(kr, report.checkins, {}), null);
  assert.equal(krHistoryValues(kr, {}, {}), null);
});

test("krHistoryValues reads a production-metric key result's own series", () => {
  const kr = report.report.objectives[1].key_results[0]; // fpy-90, first_pass_yield
  const byId = seriesIndex(metricsAnswer);
  assert.deepEqual(krHistoryValues(kr, {}, byId), [1.0, 1.0]);
});

test("krHistoryValues is null for a metric with no series (compliance.*, goal_tasks_done.*) -- never invented", () => {
  const kr = report.report.objectives[0].key_results[0]; // compliance.cra
  assert.equal(krHistoryValues(kr, {}, seriesIndex(metricsAnswer)), null);
  const goalTasks = report.report.objectives[1].key_results[2];
  assert.equal(krHistoryValues(goalTasks, {}, seriesIndex(metricsAnswer)), null);
});

// -------------------------------------------------------------- metrics

test("metricIdsForReport always includes the three production metrics, plus north star, inputs and every key result's bound metric", () => {
  const ids = metricIdsForReport(report);
  for (const id of PRODUCTION_METRIC_IDS) assert.ok(ids.includes(id));
  assert.ok(ids.includes("compliance.cra"));
  assert.ok(ids.includes("goal_tasks_done.raise-quality.sbom-pipeline-done"));
  assert.deepEqual(ids, [...ids].sort(), "sorted, for a deterministic query string");
  assert.deepEqual(ids, [...new Set(ids)], "deduplicated");
});

test("metricIdsForReport skips a manual key result's own (absent) metric", () => {
  const ids = metricIdsForReport(report);
  assert.ok(!ids.includes(undefined) && !ids.includes(null));
});

test("registryIndex and seriesIndex key by metric id", () => {
  const reg = registryIndex(metricsAnswer);
  assert.equal(reg["first_pass_yield"].unit, "ratio");
  assert.equal(reg["unit_cost"].available, false);
  const series = seriesIndex(metricsAnswer);
  assert.equal(series["throughput_week"].points.length, 4);
  assert.equal(series["compliance.cra"], undefined, "no series for a metric the daemon never computes one for");
});

test("formatUnitValue reads each unit's own shape, and a bare em dash for null", () => {
  assert.equal(formatUnitValue(0.4, "ratio"), "40.0%");
  assert.equal(formatUnitValue(5, "per_week"), "5.0/wk");
  assert.equal(formatUnitValue(3, "count"), "3");
  assert.equal(formatUnitValue(3.4, "count"), "3.4");
  assert.equal(formatUnitValue(89, "seconds"), "1m");
  assert.equal(formatUnitValue(5400, "seconds"), "1.5h");
  assert.equal(formatUnitValue(12.34, "hours"), "12.3h");
  assert.equal(formatUnitValue(0, "hours"), "0.0h");
  assert.equal(formatUnitValue(12.34, "days"), "12.3d");
  assert.equal(formatUnitValue(0, "days"), "0.0d");
  assert.equal(formatUnitValue(7, null), "7");
  assert.equal(formatUnitValue(null, "ratio"), "—");
  assert.equal(formatUnitValue(undefined, "ratio"), "—");
});

test("metricUnavailableNote reads the wire's own reason verbatim, never a manufactured zero", () => {
  const unitCost = metricsAnswer.values.find((v) => v.id === "unit_cost");
  assert.equal(metricUnavailableNote(unitCost), "not measured yet — a Run records no model, tokens or cost yet (design §12.6)");
  const computed = metricsAnswer.values.find((v) => v.id === "throughput_week");
  assert.equal(metricUnavailableNote(computed), null);
  assert.equal(metricUnavailableNote(null), null);
});

// ------------------------------------------------------------ source links

test("krSourceLink routes each metric family to its evidence level, and null for manual or an unrouted metric", () => {
  assert.equal(krSourceLink(report.report.objectives[0].key_results[0], "demo").label, "Policy");
  assert.equal(krSourceLink(report.report.objectives[1].key_results[2], "demo").label, "Tasks"); // goal_tasks_done.*
  assert.equal(krSourceLink(report.report.objectives[1].key_results[0], "demo").label, "Dashboard"); // first_pass_yield
  assert.equal(krSourceLink({ manual: true, metric: null }, "demo"), null);
  assert.equal(krSourceLink({ manual: false, metric: "unit_cost" }, "demo"), null, "an unrouted metric gets no guessed link");
  assert.match(krSourceLink({ manual: false, metric: "bench.resolve_rate.eval-a" }, "demo").href, /benchmarks$/);
});

// ------------------------------------------------------------------ table

test("tableRows flattens every objective's key results, one row each", () => {
  const rows = tableRows(report);
  assert.equal(rows.length, 5);
  assert.equal(rows[0].objective, "ship-compliant");
  assert.equal(rows[0].kr, "ship-compliant/cra-open-zero");
  assert.equal(rows[0].band, "red");
  assert.equal(rows.filter((r) => r.manual).length, 1);
});

test("tableRows is empty with no current report", () => {
  assert.deepEqual(tableRows({ report: null }), []);
});

// --------------------------------------------------------------- roadmap

test("roadmapLanes groups by lane, carries objective titles and colour indices, and flags an orphan", () => {
  const lanes = roadmapLanes(report.roadmap, report.report.objectives);
  assert.equal(lanes.now.length, 2);
  assert.equal(lanes.next.length, 1);
  assert.equal(lanes.later.length, 1);
  const sbom = lanes.now.find((i) => i.id === "sbom-pipeline");
  assert.equal(sbom.objectives.length, 2);
  assert.equal(sbom.objectives[0].title, "Every product can ship CRA-compliant");
  assert.equal(sbom.objectives[0].colorIndex, 0);
  assert.equal(sbom.objectives[1].colorIndex, 1);
  assert.ok(Math.abs(sbom.progress - (0.2916666666666667 + 0.6666666666666666) / 2) < 1e-9);
  const orphan = lanes.now.find((i) => i.id === "mystery-initiative");
  assert.equal(orphan.orphan, true);
  assert.equal(orphan.progress, null);
});

test("roadmapLanes falls an unknown lane back to later rather than dropping the item", () => {
  const lanes = roadmapLanes([{ id: "x", title: "X", lane: "sometime", objectives: [] }], []);
  assert.equal(lanes.later.length, 1);
});

test("objectiveColorVar cycles through the seven-token palette and never returns --t0", () => {
  assert.equal(objectiveColorVar(0), "var(--t1)");
  assert.equal(objectiveColorVar(6), "var(--t7)");
  assert.equal(objectiveColorVar(7), "var(--t1)", "wraps");
  assert.equal(objectiveColorVar(-1), "var(--t7)", "negative indices still land on a real token");
});

// ----------------------------------------------------------- strategy map

test("objectiveLayers puts a parentless objective in layer 0 and its aligns_to child in layer 1", () => {
  const layers = objectiveLayers(report.report.objectives);
  assert.equal(layers.length, 2);
  assert.deepEqual(layers[0].map((o) => o.objective), ["ship-compliant"]);
  assert.deepEqual(layers[1].map((o) => o.objective), ["raise-quality"]);
});

test("objectiveLayers never infinitely recurses on a cyclic aligns_to", () => {
  const cyclic = [
    { objective: "a", aligns_to: "b" },
    { objective: "b", aligns_to: "a" },
  ];
  const layers = objectiveLayers(cyclic);
  assert.ok(layers.length >= 1);
});

test("alignsToEdges only draws a connector when the parent is among the objectives on screen", () => {
  const edges = alignsToEdges(report.report.objectives);
  assert.deepEqual(edges, [{ from: "raise-quality", to: "ship-compliant" }]);
  const scoped = alignsToEdges([{ objective: "raise-quality", aligns_to: "ship-compliant" }]);
  assert.deepEqual(scoped, [], "the parent was filtered out of this scope's objectives -- no connector to draw");
});

test("danglingAlignsTo names a parent that is not among the objectives on screen", () => {
  const scoped = danglingAlignsTo([{ objective: "raise-quality", aligns_to: "ship-compliant" }]);
  assert.deepEqual(scoped, [{ objective: "raise-quality", alignsTo: "ship-compliant" }]);
  assert.deepEqual(danglingAlignsTo(report.report.objectives), [], "the parent is on screen -- not dangling");
});

test("connectorPath draws from the parent's bottom-centre to the child's top-centre, in container-relative coordinates", () => {
  const container = { left: 100, top: 50, width: 800, height: 600 };
  const parent = { left: 200, top: 100, width: 100, height: 40 }; // bottom-centre: (250, 140)
  const child = { left: 400, top: 200, width: 60, height: 30 }; // top-centre: (430, 200)
  const d = connectorPath(parent, child, container);
  assert.match(d, /^M 150 90 C /); // (250-100, 140-50)
  assert.match(d, /330 150$/); // (430-100, 200-50)
});

// --------------------------------------------------------------- orbit

test("orbitLayout places every objective and key result deterministically, with matching edges", () => {
  const layout = orbitLayout(report.report.objectives, { width: 400, height: 400 });
  assert.equal(layout.tooMany, false);
  assert.equal(layout.totalKrs, 5);
  assert.equal(layout.nodes.filter((n) => n.kind === "objective").length, 2);
  assert.equal(layout.nodes.filter((n) => n.kind === "kr").length, 5);
  assert.equal(layout.edges.length, 5);
  for (const n of layout.nodes) {
    assert.ok(Number.isFinite(n.x) && Number.isFinite(n.y));
  }
  // Deterministic: the same input lays out identically twice.
  const again = orbitLayout(report.report.objectives, { width: 400, height: 400 });
  assert.deepEqual(layout.nodes, again.nodes);
});

test("orbitLayout refuses to lay out more than ORBIT_MAX_KRS key results", () => {
  const objectives = [{ objective: "o", title: "O", key_results: Array.from({ length: ORBIT_MAX_KRS + 1 }, (_, i) => ({ kr: `o/kr${i}`, title: `KR ${i}` })) }];
  const layout = orbitLayout(objectives);
  assert.equal(layout.tooMany, true);
  assert.equal(layout.totalKrs, ORBIT_MAX_KRS + 1);
  assert.deepEqual(layout.nodes, []);
});

// -------------------------------------------------------------- findings

test("findingLabel translates every wire FindingKind, falling back to the raw spelling for an unknown one", () => {
  assert.equal(findingLabel("orphan_roadmap_item"), "Roadmap item names no objective");
  assert.equal(findingLabel("cyclic_aligns_to"), "aligns_to loops back on itself");
  assert.equal(findingLabel("something_new"), "something_new");
});

test("findingsByKind groups findings by kind", () => {
  const groups = findingsByKind(report.findings);
  assert.equal(groups.get("orphan_roadmap_item").length, 1);
});

// ------------------------------------------------------------- check-ins

test("checkinBody validates value and confidence before the round trip", () => {
  assert.deepEqual(checkinBody("obj/kr", { value: "3", confidence: "6", note: "  closer  " }), { kr: "obj/kr", value: 3, confidence: 6, note: "closer" });
  assert.deepEqual(checkinBody("obj/kr", { value: "3", confidence: "0" }), { kr: "obj/kr", value: 3, confidence: 0 });
  assert.throws(() => checkinBody("obj/kr", { value: "", confidence: "5" }), /Value is required/);
  assert.throws(() => checkinBody("obj/kr", { value: "abc", confidence: "5" }), /Value is required/);
  assert.throws(() => checkinBody("obj/kr", { value: "3", confidence: "11" }), /Confidence must be/);
  assert.throws(() => checkinBody("obj/kr", { value: "3", confidence: "-1" }), /Confidence must be/);
  assert.throws(() => checkinBody("obj/kr", { value: "3", confidence: "2.5" }), /Confidence must be/);
});

// -------------------------------------------------------------- view mode

test("normalizeViewMode falls back to map for anything it does not recognize", () => {
  assert.equal(normalizeViewMode("roadmap"), "roadmap");
  assert.equal(normalizeViewMode("orbit"), "orbit");
  assert.equal(normalizeViewMode("table"), "table");
  assert.equal(normalizeViewMode("bogus"), "map");
  assert.equal(normalizeViewMode(null), "map");
  assert.equal(normalizeViewMode(undefined), "map");
});

// ------------------------------------------------------------- empty state

test("hasDirection is false with no direction.yaml loaded", () => {
  assert.equal(hasDirection(report), true);
  assert.equal(hasDirection({ direction: null }), false);
  assert.equal(hasDirection(null), false);
});
