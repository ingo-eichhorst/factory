import test from "node:test";
import assert from "node:assert/strict";

import {
  listingScope,
  metricsUrl,
  shapeRows,
  formatValue,
  betterLabel,
  freshnessLabel,
  freshnessClass,
  statusLabel,
  consumerKindLabel,
  consumerBounds,
  consumerName,
  statusCounts,
  sourceCell,
  dependencyTarget,
  filterRows,
  countLine,
} from "../js/metrics-model.js";

// A `GET /api/metrics?scope=finance` answer, shaped the way the daemon
// sends it: `rows` carry source, freshness and consumers; `values` and
// `registry` carry the value and its definition for the same ids.
const ANSWER = {
  values: [
    { id: "reported.finance.bookings_without_beleg", value: 2, as_of: "2026-10-20T09:00:00Z" },
    { id: "fail_rate", value: null, as_of: "2026-10-21T10:00:00Z", reason: "no finished runs in the trailing 28 days" },
  ],
  registry: [
    { id: "reported.finance.bookings_without_beleg", title: "Bookings without Beleg", unit: "count", better: "lower" },
    { id: "fail_rate", title: "Failure rate", unit: "ratio", better: "lower" },
  ],
  rows: [
    {
      id: "reported.finance.bookings_without_beleg",
      source: {
        kind: "service",
        source: "finance",
        scope: "finance",
        service: "fava",
        endpoint: "127.0.0.1:8793",
        path: "/buchhaltung/extension/FactoryMetrics/metrics",
        url: "http://127.0.0.1:8793/buchhaltung/extension/FactoryMetrics/metrics",
      },
      freshness: "fresh",
      max_age: "1w",
      consumers: [
        { kind: "policy", id: "gobd/belegprinzip", scope: "finance", status: "not_met", below: 0, max_age: "1w", reasons: ["… above the allowed 0"] },
        { kind: "goal", id: "books/beleg", scope: "company", status: "not_met", below: 0 },
      ],
    },
    {
      id: "fail_rate",
      source: { kind: "built_in" },
      freshness: "no_data",
      consumers: [{ kind: "quality", id: "reliability/rarely-fails", scope: "finance", status: "no_data", below: 0.2 }],
    },
  ],
  findings: [],
};

// ------------------------------------------------------------------ scope

test("listingScope keeps the rail's selection and, with none, falls back to the root scope", () => {
  const scopes = [
    { name: "finance", path: "/inst/projects/finance" },
    { name: "company", path: "/inst" },
  ];
  assert.equal(listingScope("finance", scopes, "/inst"), "finance");
  assert.equal(listingScope(null, scopes, "/inst"), "company");
  assert.equal(listingScope(null, [{ name: "company", path: "." }], ""), "company");
  assert.equal(listingScope(null, [{ name: "finance", path: "/inst/projects/finance" }], "/inst"), null);
  assert.equal(listingScope(null, [], "/inst"), null);
});

test("metricsUrl asks for a scope's listing with no ids", () => {
  assert.equal(metricsUrl("finance"), "/api/metrics?scope=finance");
  assert.equal(metricsUrl("a b/c"), "/api/metrics?scope=a%20b%2Fc");
});

// ------------------------------------------------------------------- rows

test("shapeRows joins each row with its value and definition, in the daemon's order", () => {
  const rows = shapeRows(ANSWER);
  assert.equal(rows.length, 2, "exactly the rows the daemon listed");
  assert.deepEqual(
    rows.map((r) => r.id),
    ["reported.finance.bookings_without_beleg", "fail_rate"],
  );
  const [beleg, fails] = rows;
  assert.equal(beleg.title, "Bookings without Beleg");
  assert.equal(beleg.value, 2);
  assert.equal(beleg.better, "lower");
  assert.equal(beleg.maxAge, "1w");
  assert.equal(beleg.consumers.length, 2);
  assert.equal(fails.value, null, "None stays None, never 0");
  assert.equal(fails.reason, "no finished runs in the trailing 28 days");
  assert.equal(fails.freshness, "no_data");
});

test("shapeRows of nothing, or of an answer with no rows, is no rows", () => {
  assert.deepEqual(shapeRows(null), []);
  assert.deepEqual(shapeRows({ values: [], registry: [] }), []);
});

test("a row whose definition is missing falls back to its id rather than a blank", () => {
  const rows = shapeRows({ values: [], registry: [], rows: [{ id: "x", source: { kind: "built_in" }, freshness: "no_data" }] });
  assert.equal(rows[0].title, "x");
  assert.equal(rows[0].value, null);
  assert.deepEqual(rows[0].consumers, []);
});

// ------------------------------------------------------------------ labels

test("formatValue reads a ratio as a percentage, a count as a whole number, and no value as None", () => {
  assert.equal(formatValue(0.987, "ratio"), "98.7%");
  assert.equal(formatValue(3, "count"), "3");
  assert.equal(formatValue(2.5, "hours"), "2.50");
  assert.equal(formatValue(0, "count"), "0", "a real zero is a zero");
  assert.equal(formatValue(null, "count"), "None");
  assert.equal(formatValue(Number.NaN, "count"), "None");
});

test("every freshness, status and consumer kind has a label, and an unknown one falls through", () => {
  for (const f of ["fresh", "stale", "no_data", "no_window"]) assert.ok(freshnessLabel(f), f);
  for (const s of ["met", "not_met", "stale", "no_data", "draft"]) assert.ok(statusLabel(s), s);
  assert.equal(consumerKindLabel("policy"), "control");
  assert.equal(consumerKindLabel("quality"), "scenario");
  assert.equal(consumerKindLabel("goal"), "key result");
  assert.equal(statusLabel("bogus"), "bogus");
  assert.equal(freshnessLabel(""), "");
});

test("freshness reads in the shared badge colours", () => {
  assert.equal(freshnessClass("fresh"), "met");
  assert.equal(freshnessClass("stale"), "stale");
  assert.equal(freshnessClass("no_data"), "no_data");
  assert.equal(freshnessClass("no_window"), "draft");
});

test("betterLabel names the direction only when there is one", () => {
  assert.match(betterLabel("higher"), /higher/);
  assert.match(betterLabel("lower"), /lower/);
  assert.equal(betterLabel(""), "");
});

// --------------------------------------------------------------- consumers

test("a consumer is named by kind, id and scope, with its own bounds", () => {
  const [policy, goal] = ANSWER.rows[0].consumers;
  assert.equal(consumerName(policy), "control gobd/belegprinzip @ finance");
  assert.equal(consumerName({ kind: "goal", id: "books/beleg" }), "key result books/beleg");
  assert.equal(consumerBounds(policy), "≤ 0 · max_age 1w");
  assert.equal(consumerBounds(goal), "≤ 0");
  assert.equal(consumerBounds({ above: 0.98 }), "≥ 0.98");
});

test("statusCounts tallies a row's verdicts", () => {
  assert.deepEqual(statusCounts(ANSWER.rows[0].consumers), { not_met: 2 });
  assert.deepEqual(statusCounts([]), {});
});

// ------------------------------------------------------------------ source

test("sourceCell names built-in, file and service sources, and a refused one as bad", () => {
  assert.equal(sourceCell({ kind: "built_in" }).label, "built-in");
  const file = sourceCell({ kind: "file", source: "finance", scope: "finance", path: "data/finance/metrics.json" });
  assert.equal(file.label, "file · finance");
  assert.equal(file.detail, "data/finance/metrics.json");
  assert.equal(file.service, null);
  const service = sourceCell(ANSWER.rows[0].source);
  assert.equal(service.label, "service · fava");
  assert.equal(service.detail, "http://127.0.0.1:8793/buchhaltung/extension/FactoryMetrics/metrics");
  const refused = sourceCell({ kind: "unresolved", source: "finance", scope: "finance", service: "fava", reason: "no loopback endpoint" });
  assert.equal(refused.bad, true);
  assert.match(refused.label, /fava/);
  assert.equal(refused.detail, "no loopback endpoint");
  assert.equal(sourceCell({ kind: "unresolved", source: "nobody", reason: "no scope declares" }).label, "unresolved");
});

test("a service source -- answered or refused -- links to its declaring scope's Dependencies row", () => {
  assert.deepEqual(dependencyTarget(ANSWER.rows[0].source), { scope: "finance", service: "fava" });
  assert.deepEqual(
    dependencyTarget({ kind: "unresolved", source: "finance", scope: "finance", service: "fava", reason: "x" }),
    { scope: "finance", service: "fava" },
  );
  assert.equal(dependencyTarget({ kind: "built_in" }), null);
  assert.equal(dependencyTarget({ kind: "file", source: "f", scope: "finance", path: "m.json" }), null);
  assert.equal(dependencyTarget({ kind: "unresolved", source: "nobody", reason: "x" }), null);
});

// ------------------------------------------------------------------ filter

test("filterRows matches every word against id, title, source and consumers", () => {
  const rows = shapeRows(ANSWER);
  assert.equal(filterRows(rows, "").length, 2);
  assert.deepEqual(filterRows(rows, "fava").map((r) => r.id), ["reported.finance.bookings_without_beleg"]);
  assert.deepEqual(filterRows(rows, "belegprinzip").map((r) => r.id), ["reported.finance.bookings_without_beleg"]);
  assert.deepEqual(filterRows(rows, "scenario failure").map((r) => r.id), ["fail_rate"]);
  assert.deepEqual(filterRows(rows, "fava failure"), []);
});

test("countLine counts metrics", () => {
  assert.equal(countLine([]), "0 metrics");
  assert.equal(countLine([{}]), "1 metric");
  assert.equal(countLine([{}, {}]), "2 metrics");
});
