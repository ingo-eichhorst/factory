import test from "node:test";
import assert from "node:assert/strict";

import {
  STATUS_ORDER,
  attributeLabel,
  attributeTitles,
  bulletGeometry,
  canRemediate,
  cellLabel,
  decisionLink,
  describeMeasure,
  findingLabel,
  findingsByKind,
  focusScope,
  formatNumber,
  hasProfiles,
  heatmapRows,
  idTag,
  importanceDifficultyGrid,
  maxLevel,
  measureKind,
  normalizeViewMode,
  openTaskFor,
  remediateBody,
  scenarioSentence,
  sentenceText,
  seriesValues,
  sparkGeometry,
  tableRows,
  taskHref,
  tradeoffMatrix,
  utilityTree,
  valueDomain,
  worstStatus,
} from "../js/quality-model.js";

// A real `GET /api/quality?scope=demo` answer's `.report` (after `api()`
// unwraps `{status, data}`), captured from a throwaway daemon, not a shape
// invented by reading the Rust: the root binding `examples/quality/baseline.yaml`
// (`quality: [baseline]`) and `examples/policies/cra.yaml`, a `demo` scope
// binding `examples/quality/daemon-service.yaml`, a `quality-gate` shell task
// run to `done` there, a `flaky` one to `failed` (so `scrap_rate` and
// `first_pass_yield` have values to miss their bounds by), and
// `factory quality remediate maintainability.modifiability/right-first-time
// --scope demo` run once (the `open_tasks` entry). Trimmed only in the
// catalogue (each characteristic keeps just the sub-characteristics this
// scope declares; all nine columns stay) -- the daemon was hours old, so each
// production metric's series has one point.
const report = {
  scope: "demo",
  scopes: [
    {
      scope: "demo",
      profiles: ["baseline", "daemon-service"],
      attributes: [
        {
          id: "security.integrity",
          characteristic: "security",
          importance: "H",
          difficulty: "M",
          declared_at: {"scope": "dev", "profile": "baseline"},
          status: "no_data",
          scenarios: [
            {"id": "cra-controls-evidenced", "kind": "change", "source": "a release manager", "stimulus": "a release is cut", "artifact": "every shipped product", "environment": "normal operation", "response": "the CRA's controls are evidenced, not just claimed", "measure": {"metric": "compliance.cra", "above": 0.9}, "declared_at": {"scope": "dev", "profile": "baseline"}, "status": "no_data", "reasons": ["compliance.cra could not be computed: no catalogue loaded for framework \"cra\""], "as_of": "2026-09-25T11:07:30.130520Z"},
          ],
        },
        {
          id: "reliability.recoverability",
          characteristic: "reliability",
          importance: "H",
          difficulty: "M",
          declared_at: {"scope": "dev", "profile": "baseline"},
          status: "not_met",
          scenarios: [
            {"id": "little-work-scrapped", "kind": "usage", "source": "any agent", "stimulus": "a run goes wrong part way through", "artifact": "the task it was working on", "environment": "normal operation", "response": "the task is retried or recovered rather than scrapped", "measure": {"metric": "scrap_rate", "below": 0.1, "max_age": "1w"}, "declared_at": {"scope": "dev", "profile": "baseline"}, "status": "not_met", "reasons": ["scrap_rate = 0.5, above the allowed 0.1"], "value": 0.5, "as_of": "2026-09-25T11:07:30.132191Z"},
            {"id": "daemon-restart", "kind": "usage", "source": "launchd", "stimulus": "daemon restarted while 3 runs are active", "artifact": "factory-daemon", "environment": "normal operation", "response": "runs resume reporting; no run is lost or double-dispatched", "measure": {"metric": "scrap_rate", "below": 0.05, "max_age": "1w"}, "declared_at": {"scope": "demo", "profile": "daemon-service"}, "status": "not_met", "reasons": ["scrap_rate = 0.5, above the allowed 0.05"], "value": 0.5, "as_of": "2026-09-25T11:07:30.132191Z"},
          ],
        },
        {
          id: "maintainability.modifiability",
          characteristic: "maintainability",
          importance: "H",
          difficulty: "H",
          declared_at: {"scope": "demo", "profile": "daemon-service"},
          status: "not_met",
          scenarios: [
            {"id": "agent-diff-health", "kind": "change", "source": "a coding agent", "stimulus": "an agent's change lands on main", "artifact": "the Rust workspace", "environment": "normal development", "response": "no new clippy warnings, duplication does not rise", "measure": {"check": "task", "task": "quality-gate", "max_age": "1w"}, "declared_at": {"scope": "demo", "profile": "daemon-service"}, "status": "met", "reasons": ["task: `quality-gate` (b5647d17-dbc3-4ade-9a01-0dde1c694fcf) run da6cc6b7-b629-4567-8613-09cde0f1e04f done at 2026-09-25 11:03:43.680502 UTC"], "refs": [{"kind": "task", "id": "b5647d17-dbc3-4ade-9a01-0dde1c694fcf"}, {"kind": "run", "id": "da6cc6b7-b629-4567-8613-09cde0f1e04f"}]},
            {"id": "right-first-time", "kind": "change", "stimulus": "an agent is handed a change to make", "response": "it lands on the first attempt", "measure": {"metric": "first_pass_yield", "above": 0.8}, "declared_at": {"scope": "demo", "profile": "daemon-service"}, "status": "not_met", "reasons": ["first_pass_yield = 0.3333333333333333, below the required 0.8"], "value": 0.3333333333333333, "as_of": "2026-09-25T11:07:30.132191Z"},
          ],
        },
        {
          id: "performance-efficiency.time-behaviour",
          characteristic: "performance-efficiency",
          importance: "M",
          difficulty: "L",
          declared_at: {"scope": "demo", "profile": "daemon-service"},
          status: "no_data",
          scenarios: [
            {"id": "bench-latency", "measure": {"metric": "bench.resolve_rate.smoke", "above": 0.9}, "declared_at": {"scope": "demo", "profile": "daemon-service"}, "status": "no_data", "reasons": ["bench.resolve_rate.smoke could not be computed: no settled bench run for dataset \"smoke\""], "as_of": "2026-09-25T11:07:30.130520Z"},
          ],
        },
        {
          id: "security.confidentiality",
          characteristic: "security",
          importance: "H",
          difficulty: "M",
          declared_at: {"scope": "demo", "profile": "daemon-service"},
          status: "met",
          scenarios: [
            {"id": "agents-sandboxed", "kind": "usage", "source": "any agent", "stimulus": "an agent is dispatched", "artifact": "the agent's session", "environment": "normal operation", "response": "it runs inside a sandbox, never on the bare host", "measure": {"check": "sandbox"}, "declared_at": {"scope": "demo", "profile": "daemon-service"}, "status": "met", "reasons": ["sandbox: no agent declared in this scope"]},
          ],
        },
        {
          id: "interaction-capability.transparency",
          characteristic: "interaction-capability",
          importance: "M",
          difficulty: "M",
          declared_at: {"scope": "demo", "profile": "daemon-service"},
          status: "draft",
          scenarios: [
            {"id": "failure-explained", "kind": "usage", "source": "an operator", "stimulus": "a run fails", "artifact": "the run's report", "response": "the report says why, in one sentence a person can act on", "declared_at": {"scope": "demo", "profile": "daemon-service"}, "status": "draft", "reasons": ["no response measure yet"]},
          ],
        },
      ],
      tradeoffs: [{"between": ["security.confidentiality", "performance-efficiency.time-behaviour"], "point": "every agent runs in a sandbox; start-up cost accepted", "decision": "knowledge/adr-sandbox.md", "declared_at": {"scope": "demo", "profile": "daemon-service"}}],
      open_tasks: {"maintainability.modifiability/right-first-time": "b7732c3a-2828-41ef-9972-bcf917d4dc56"},
    },
  ],
  findings: [],
  catalogue: [
    {"id": "functional-suitability", "title": "Functional suitability", "subs": []},
    {"id": "performance-efficiency", "title": "Performance efficiency", "subs": [{"id": "time-behaviour", "title": "Time behaviour", "standard": "iso-25010"}]},
    {"id": "compatibility", "title": "Compatibility", "subs": []},
    {"id": "interaction-capability", "title": "Interaction capability", "subs": [{"id": "transparency", "title": "Transparency", "standard": "iso-25059"}]},
    {"id": "reliability", "title": "Reliability", "subs": [{"id": "recoverability", "title": "Recoverability", "standard": "iso-25010"}]},
    {"id": "security", "title": "Security", "subs": [{"id": "confidentiality", "title": "Confidentiality", "standard": "iso-25010"}, {"id": "integrity", "title": "Integrity", "standard": "iso-25010"}]},
    {"id": "maintainability", "title": "Maintainability", "subs": [{"id": "modifiability", "title": "Modifiability", "standard": "iso-25010"}]},
    {"id": "flexibility", "title": "Flexibility", "subs": []},
    {"id": "safety", "title": "Safety", "subs": []},
  ],
  series: [{"id": "scrap_rate", "points": [["2026-09-25", 0.5]]}, {"id": "first_pass_yield", "points": [["2026-09-25", 0.3333333333333333]]}],
};

const demo = report.scopes[0];
const attr = (id) => demo.attributes.find((a) => a.id === id);
const scenario = (a, s) => attr(a).scenarios.find((x) => x.id === s);

// ------------------------------------------------------------------ status

test("worstStatus follows ScenarioStatus' own order, reads nothing as draft, and ranks an unknown spelling worst", () => {
  assert.deepEqual(STATUS_ORDER, ["met", "draft", "no_data", "stale", "not_met"]);
  assert.equal(worstStatus(["met", "no_data", "draft"]), "no_data");
  assert.equal(worstStatus(["met", "stale"]), "stale");
  assert.equal(worstStatus(["stale", "not_met", "met"]), "not_met");
  assert.equal(worstStatus([]), "draft", "an attribute with no scenarios is a stated concern, never met");
  assert.equal(worstStatus(["met", "on_fire"]), "on_fire", "never read a status this build does not know as good news");
});

test("Create task: always for not_met and stale, for a no_data gap a task could close, never for one only a profile edit can", () => {
  assert.equal(canRemediate(scenario("reliability.recoverability", "daemon-restart"), []), true); // not_met
  assert.equal(canRemediate({ status: "stale", measure: { metric: "scrap_rate", below: 0.1 } }, []), true);
  assert.equal(canRemediate(scenario("security.confidentiality", "agents-sandboxed"), []), false, "met");
  assert.equal(canRemediate(scenario("interaction-capability.transparency", "failure-explained"), []), false, "draft");
  // no_data because the bench was never run -- a task can run it.
  const bench = scenario("performance-efficiency.time-behaviour", "bench-latency");
  assert.equal(canRemediate(bench, []), true);
  // The daemon's own `unfixable_by_a_task` cases, read off the wire.
  assert.equal(canRemediate({ status: "no_data", measure: { check: "attestation" } }, []), false);
  assert.equal(canRemediate({ status: "no_data", measure: { check: "task", task: "gate" } }, []), true);
  assert.equal(canRemediate({ status: "no_data", measure: { metric: "quality.reliability", above: 1 } }, []), false);
  const unavailable = [{ kind: "unavailable_metric", subject: "p.yaml", detail: "attribute a scenario s names metric unit_cost which is not available yet: §12.6" }];
  assert.equal(canRemediate({ status: "no_data", measure: { metric: "unit_cost", below: 1 } }, unavailable), false);
  assert.equal(canRemediate({ status: "no_data", measure: { metric: "unit", below: 1 } }, unavailable), true, "a whole metric id, not a prefix of one");
  const unknown = [{ kind: "unknown_metric", subject: "p.yaml", detail: "attribute a scenario s names unknown metric fail_rate" }];
  assert.equal(canRemediate({ status: "no_data", measure: { metric: "fail_rate", below: 1 } }, unknown), false);
});

test("maxLevel, idTag", () => {
  assert.equal(maxLevel(["L", "M"]), "M");
  assert.equal(maxLevel(["M", "H", "L"]), "H");
  assert.equal(maxLevel([]), null);
  assert.equal(idTag(attr("maintainability.modifiability")), "(H,H)");
});

test("attribute titles come from the report's own catalogue, with the raw id as the fallback", () => {
  assert.equal(attributeLabel("reliability.recoverability", report.catalogue), "Reliability › Recoverability");
  assert.equal(attributeLabel("safety", report.catalogue), "Safety");
  assert.equal(attributeTitles("interaction-capability.transparency", report.catalogue).standard, "iso-25059");
  assert.equal(attributeLabel("warp.drive", report.catalogue), "warp › drive");
});

// ----------------------------------------------------------------- heatmap

test("heatmapRows: nine columns in the standard's order, the worst status and highest importance per cell, undeclared cells blank", () => {
  const { columns, rows } = heatmapRows(report);
  assert.deepEqual(columns.map((c) => c.id), [
    "functional-suitability", "performance-efficiency", "compatibility", "interaction-capability",
    "reliability", "security", "maintainability", "flexibility", "safety",
  ]);
  assert.equal(rows.length, 1);
  const cell = (id) => rows[0].cells.find((c) => c.characteristic === id);
  // security: integrity (H, no_data) and confidentiality (H, met) -- the worst, never an average.
  assert.deepEqual(
    { declared: cell("security").declared, status: cell("security").status, importance: cell("security").importance },
    { declared: true, status: "no_data", importance: "H" },
  );
  assert.equal(cell("maintainability").status, "not_met");
  assert.equal(cell("interaction-capability").status, "draft");
  assert.equal(cell("performance-efficiency").importance, "M");
  for (const id of ["functional-suitability", "compatibility", "flexibility", "safety"]) {
    assert.equal(cell(id).declared, false, `${id} is not declared here`);
    assert.equal(cell(id).status, null, "an undeclared cell has no status at all -- it is never failing");
  }
  assert.match(cellLabel("demo", columns[8], cell("safety")), /not declared/);
  assert.match(cellLabel("demo", columns[5], cell("security")), /no data, high importance/);
});

test("heatmapRows keeps one row per listed scope, in the report's order, and no score anywhere", () => {
  const two = { ...report, scopes: [demo, { scope: "web", profiles: ["web-app"], attributes: [], tradeoffs: [] }] };
  const { rows } = heatmapRows(two);
  assert.deepEqual(rows.map((r) => r.scope), ["demo", "web"]);
  assert.ok(rows[1].cells.every((c) => !c.declared));
  for (const r of rows) assert.deepEqual(Object.keys(r).sort(), ["cells", "profiles", "scope"], "no score field sneaks onto a row");
});

// ------------------------------------------------------------ utility tree

test("utilityTree groups attributes by characteristic in catalogue order, each group the worst of its own", () => {
  const tree = utilityTree(demo, report.catalogue);
  assert.deepEqual(tree.map((g) => g.characteristic), [
    "performance-efficiency", "interaction-capability", "reliability", "security", "maintainability",
  ]);
  const security = tree.find((g) => g.characteristic === "security");
  assert.deepEqual(security.attributes.map((a) => a.title), ["Integrity", "Confidentiality"]);
  assert.equal(security.status, "no_data");
  assert.equal(tree.find((g) => g.characteristic === "interaction-capability").attributes[0].standard, "iso-25059");
});

test("openTaskFor reads the daemon's own <attribute>/<scenario> key, and tolerates a report with no open_tasks at all", () => {
  assert.equal(openTaskFor(demo, "maintainability.modifiability", "right-first-time"), "b7732c3a-2828-41ef-9972-bcf917d4dc56");
  assert.equal(openTaskFor(demo, "reliability.recoverability", "daemon-restart"), null);
  const { open_tasks, ...bare } = demo;
  assert.equal(openTaskFor(bare, "maintainability.modifiability", "right-first-time"), null);
});

test("remediateBody and taskHref", () => {
  assert.deepEqual(remediateBody("demo", "reliability.recoverability", "daemon-restart"), {
    scope: "demo", attribute: "reliability.recoverability", scenario: "daemon-restart",
  });
  assert.match(taskHref("demo", "b7732c3a"), /task\/b7732c3a$/);
});

// ---------------------------------------------------------------- sentence

test("scenarioSentence renders all six parts as one sentence, each authored part tagged", () => {
  const s = scenario("reliability.recoverability", "daemon-restart");
  assert.equal(
    sentenceText(s),
    "When daemon restarted while 3 runs are active (from launchd) on factory-daemon during normal operation, " +
      "runs resume reporting; no run is lost or double-dispatched. Measured by scrap_rate ≤ 0.05, no older than 1w.",
  );
  const parts = scenarioSentence(s).filter((x) => x.part).map((x) => x.part);
  assert.deepEqual(parts, ["stimulus", "source", "artifact", "environment", "response", "measure"]);
});

test("scenarioSentence degrades: stimulus and response only, measure only, and a draft that says it is one", () => {
  assert.equal(
    sentenceText(scenario("maintainability.modifiability", "right-first-time")),
    "When an agent is handed a change to make, it lands on the first attempt. Measured by first_pass_yield ≥ 0.8.",
  );
  assert.equal(
    sentenceText(scenario("performance-efficiency.time-behaviour", "bench-latency")),
    "Measured by bench.resolve_rate.smoke ≥ 0.9.",
  );
  assert.match(sentenceText(scenario("interaction-capability.transparency", "failure-explained")), /No response measure yet — a draft, never met\.$/);
});

test("describeMeasure: a metric's bounds are inclusive; a check measure is policy-model's own describeCheck", () => {
  assert.equal(describeMeasure({ metric: "x", above: 0.2, below: 0.8 }), "x between 0.2 and 0.8");
  assert.equal(describeMeasure(scenario("maintainability.modifiability", "agent-diff-health").measure), "task quality-gate (max_age 1w)");
  assert.equal(describeMeasure(scenario("security.confidentiality", "agents-sandboxed").measure), "sandbox");
  assert.equal(describeMeasure(null), null);
  assert.equal(measureKind({ metric: "x", above: 1 }), "continual");
  assert.equal(measureKind({ check: "sandbox" }), "triggered");
});

test("formatNumber never invents a unit or a trailing run of digits", () => {
  assert.equal(formatNumber(0.3333333333333333), "0.333");
  assert.equal(formatNumber(0.5), "0.5");
  assert.equal(formatNumber(12), "12");
  assert.equal(formatNumber(1234.567), "1235");
  assert.equal(formatNumber(null), "—");
});

// ------------------------------------------------------------ bullet chart

test("bulletGeometry: a ratio sits on 0..1, the pass region is where the value has to land, the bound is a tick", () => {
  const s = scenario("reliability.recoverability", "daemon-restart"); // scrap_rate 0.5, below 0.05
  const g = bulletGeometry(s.measure, s.value, seriesValues(report, "scrap_rate"), { width: 200, height: 20 });
  assert.deepEqual(g.domain, [0, 1]);
  assert.deepEqual(g.pass, { x: 0, w: 10 });
  assert.equal(g.bar.w, 100);
  assert.deepEqual(g.ticks.map((t) => [t.kind, t.x]), [["below", 10]]);

  const above = scenario("maintainability.modifiability", "right-first-time"); // first_pass_yield 0.333, above 0.8
  const h = bulletGeometry(above.measure, above.value, null, { width: 200, height: 20 });
  assert.deepEqual(h.pass, { x: 160, w: 40 });
});

test("bulletGeometry draws nothing for a check measure or a missing value -- a missing value is never a zero", () => {
  const check = scenario("maintainability.modifiability", "agent-diff-health");
  assert.equal(bulletGeometry(check.measure, check.value, null), null);
  const noValue = scenario("security.integrity", "cra-controls-evidenced");
  assert.equal(noValue.value, undefined);
  assert.equal(bulletGeometry(noValue.measure, noValue.value, null), null);
});

test("valueDomain widens past 1 only when something on it does, and holds zero, bounds and history", () => {
  assert.deepEqual(valueDomain({ metric: "throughput_week", above: 1 }, 0.4, [0, 0.2]), [0, 1]);
  const [lo, hi] = valueDomain({ metric: "throughput_week", above: 3 }, 5, [2, 8]);
  assert.equal(lo, 0);
  assert.ok(hi > 8);
});

test("sparkGeometry needs two points and shares the bullet's value axis", () => {
  assert.equal(seriesValues(report, "scrap_rate").length, 1);
  assert.equal(sparkGeometry(seriesValues(report, "scrap_rate"), [0, 1]), null, "one point is a value, not a trend");
  assert.equal(seriesValues(report, "bench.resolve_rate.smoke"), null, "never invented for a metric with no series");
  const g = sparkGeometry([0, 0.5, 1], [0, 1], { width: 100, height: 10 });
  assert.equal(g.points, "0.0,10.0 50.0,5.0 100.0,0.0");
  assert.equal(g.y(0.05), 9.5);
});

// ---------------------------------------------------------------- tradeoffs

test("tradeoffMatrix is symmetric and keeps the declared order, appending an undeclared attribute a point names", () => {
  const m = tradeoffMatrix(demo);
  assert.equal(m.count, 1);
  assert.deepEqual(m.axes.map((a) => a.id), demo.attributes.map((a) => a.id));
  const a = "security.confidentiality";
  const b = "performance-efficiency.time-behaviour";
  assert.equal(m.cells.get(`${a}|${b}`), m.cells.get(`${b}|${a}`));
  assert.match(m.cells.get(`${a}|${b}`)[0].point, /start-up cost accepted/);

  const odd = { ...demo, tradeoffs: [{ between: [a, "safety.fail-safe"], point: "p" }] };
  const n = tradeoffMatrix(odd);
  assert.deepEqual(n.axes.at(-1), { id: "safety.fail-safe", declared: false });
});

test("decisionLink: a knowledge page deep-links by its page id, a URL opens as itself, anything else stays text", () => {
  const k = decisionLink("knowledge/adr-sandbox.md", "demo");
  assert.equal(k.kind, "knowledge");
  assert.equal(k.label, "adr-sandbox");
  assert.match(k.href, /^#demo\/.*knowledge\/adr-sandbox$/);
  assert.match(decisionLink(".factory/knowledge/decisions/adr-7.md", null).href, /knowledge\/decisions\/adr-7$/);
  assert.equal(decisionLink("https://example.com/d/42", "demo").kind, "external");
  assert.deepEqual(decisionLink("the March offsite", "demo"), { kind: "text", label: "the March offsite", href: null });
  assert.equal(decisionLink(undefined, "demo"), null);
});

// ------------------------------------------------- importance × difficulty

test("importanceDifficultyGrid: nine cells, H importance first, the (H,H) corner hot", () => {
  const cells = importanceDifficultyGrid(demo);
  assert.equal(cells.length, 9);
  assert.deepEqual(cells.slice(0, 3).map((c) => `${c.importance}${c.difficulty}`), ["HL", "HM", "HH"]);
  assert.deepEqual(cells.filter((c) => c.hot).map((c) => c.attributes.map((a) => a.id)), [["maintainability.modifiability"]]);
  const hm = cells.find((c) => c.importance === "H" && c.difficulty === "M");
  assert.deepEqual(hm.attributes.map((a) => a.id), ["security.integrity", "reliability.recoverability", "security.confidentiality"]);
  assert.equal(cells.filter((c) => !c.attributes.length).length, 5, "empty cells stay -- a grid that dropped them would stop being a grid");
});

// -------------------------------------------------------------------- table

test("tableRows: one row per scenario, everything the pictures show", () => {
  const rows = tableRows(report);
  assert.equal(rows.length, demo.attributes.reduce((n, a) => n + a.scenarios.length, 0));
  const r = rows.find((x) => x.scenario === "daemon-restart");
  assert.equal(r.label, "Reliability › Recoverability");
  assert.equal(r.status, "not_met");
  assert.equal(r.value, 0.5);
  assert.equal(r.measure, "scrap_rate ≤ 0.05, no older than 1w");
  assert.equal(rows.find((x) => x.scenario === "failure-explained").measure, null);

  const empty = { ...report, scopes: [{ ...demo, attributes: [{ ...attr("security.integrity"), scenarios: [] }] }] };
  assert.equal(tableRows(empty)[0].scenario, null, "a declared attribute with no scenarios still gets a row");
});

// ----------------------------------------------------------- odds and ends

test("findings are labelled, with the raw kind as the fallback, and grouped", () => {
  assert.equal(findingLabel("too_many_attributes"), "More than seven attributes");
  assert.equal(findingLabel("brand_new_kind"), "brand_new_kind");
  const g = findingsByKind([{ kind: "loosening", subject: "a", detail: "x" }, { kind: "loosening", subject: "b", detail: "y" }]);
  assert.equal(g.get("loosening").length, 2);
});

test("normalizeViewMode, focusScope, hasProfiles", () => {
  assert.equal(normalizeViewMode("tree"), "tree");
  assert.equal(normalizeViewMode("radar"), "heatmap", "no radar view, whatever storage says");
  assert.equal(focusScope(report, "demo", null), "demo");
  assert.equal(focusScope(report, "gone", null), "demo");
  assert.equal(focusScope({ scopes: [] }, "demo", null), null);
  assert.equal(hasProfiles(report), true);
  assert.equal(hasProfiles({ scopes: [], findings: [], catalogue: [] }), false);
});
