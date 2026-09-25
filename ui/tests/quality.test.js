import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { state } from "../js/core.js";
import { readHash, setRouter } from "../js/scopes.js";

// Nothing this view imports listens on the document as it loads, but the
// bare shim is kept anyway, the same as `goals.test.js`, so a later import
// that does cannot break this file in a confusing way.
const bare = { addEventListener() {}, getElementById: () => null };
globalThis.document = bare;
const { loadQuality, renderQuality } = await import("../js/quality.js");

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const wiring = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");

test("L6 Direction's Quality tab sits between Goals and Policy, and the level's sub-label says so", () => {
  assert.match(page, /id="tab-quality"[^>]*>Quality<\/button>/);
  assert.match(page, /id="view-quality"/);
  const goals = page.indexOf('id="tab-goals"');
  const quality = page.indexOf('id="tab-quality"');
  const policy = page.indexOf('id="tab-policy"');
  assert.ok(goals < quality && quality < policy, "Goals, then Quality, then Policy");
  assert.match(page, /<span class="lv-sub">Goals, quality, policy, scenarios<\/span>/);
  assert.match(wiring, /dir:\s*\["goals",\s*"quality",\s*"policy",\s*"scenarios"\]/);
  assert.match(wiring, /quality:\s*\{\s*onShow:\s*loadQuality\s*\}/);
});

test("the tab reloads on quality_changed, on a run settling and on a quality= task -- and never polls", () => {
  assert.match(wiring, /ev\.type === "quality_changed"/);
  assert.match(wiring, /ev\.type === "run_updated" && TERMINAL\.includes\(ev\.run\.status\)/);
  assert.match(wiring, /ev\.task\.labels && ev\.task\.labels\.quality/);
  assert.match(wiring, /reloadQuality\(\)/);
  const view = readFileSync(new URL("../js/quality.js", import.meta.url), "utf8");
  assert.doesNotMatch(view, /setInterval/);
});

test("the empty state names the file, the config key and the README section", () => {
  assert.match(page, /\.factory\/quality\/&lt;profile&gt;\.yaml/);
  assert.match(page, /quality: \[&lt;profile&gt;\]/);
  assert.match(page, /README\.md#quality-attributes/);
});

test("no radar chart and no overall score anywhere in the tab", () => {
  const view = readFileSync(new URL("../js/quality.js", import.meta.url), "utf8");
  const model = readFileSync(new URL("../js/quality-model.js", import.meta.url), "utf8");
  for (const src of [view, model]) {
    assert.doesNotMatch(src, /polygon/i, "a radar is a polygon; nothing here draws one");
    assert.doesNotMatch(src, /\bscore\s*[:=]/i);
  }
});

test("a link with a level segment and an old bare link both land on the Quality page", () => {
  setRouter({ pages: ["dashboard", "goals", "quality", "policy"], redirects: {} });
  globalThis.location = { hash: "#all/dir/quality" };
  assert.equal(readHash().page, "quality");
  globalThis.location = { hash: "#all/quality" };
  assert.equal(readHash().page, "quality");
  delete globalThis.location;
});

// A trimmed `GET /api/quality` answer's `.report`, the same capture
// `quality-model.test.js` describes, cut to one scenario per status the
// view draws differently, plus a second scope that declares less.
function fixtureReport() {
  const cat = (id, title, subs = []) => ({ id, title, subs: subs.map(([sid, st]) => ({ id: sid, title: st, standard: "iso-25010" })) });
  return {
    scopes: [
      {
        scope: "demo",
        profiles: ["baseline", "daemon-service"],
        attributes: [
          {
            id: "reliability.recoverability", characteristic: "reliability", importance: "H", difficulty: "M",
            declared_at: { scope: "demo", profile: "daemon-service" }, status: "not_met",
            scenarios: [{
              id: "daemon-restart", kind: "usage", source: "launchd", stimulus: "daemon restarted while 3 runs are active",
              artifact: "factory-daemon", environment: "normal operation", response: "runs resume reporting",
              measure: { metric: "scrap_rate", below: 0.05, max_age: "1w" },
              declared_at: { scope: "demo", profile: "daemon-service" }, status: "not_met",
              reasons: ["scrap_rate = 0.5, above the allowed 0.05"], value: 0.5, as_of: "2026-09-25T11:04:06Z",
            }],
          },
          {
            id: "maintainability.modifiability", characteristic: "maintainability", importance: "H", difficulty: "H",
            declared_at: { scope: "demo", profile: "daemon-service" }, status: "not_met",
            scenarios: [{
              id: "right-first-time", kind: "change", stimulus: "an agent is handed a change to make", response: "it lands on the first attempt",
              measure: { metric: "first_pass_yield", above: 0.8 }, declared_at: { scope: "demo", profile: "daemon-service" },
              status: "not_met", reasons: ["first_pass_yield = 0.333, below the required 0.8"], value: 0.333,
            }],
          },
          {
            id: "security.integrity", characteristic: "security", importance: "H", difficulty: "M",
            declared_at: { scope: "dev", profile: "baseline" }, status: "no_data",
            scenarios: [{
              id: "cra-controls-evidenced", measure: { metric: "compliance.cra", above: 0.9 },
              declared_at: { scope: "dev", profile: "baseline" }, status: "no_data",
              reasons: ["compliance.cra could not be computed"],
            }],
          },
        ],
        tradeoffs: [{
          between: ["security.integrity", "reliability.recoverability"], point: "signed releases slow a restart",
          decision: "knowledge/adr-sandbox.md", declared_at: { scope: "demo", profile: "daemon-service" },
        }],
        open_tasks: { "maintainability.modifiability/right-first-time": "b7732c3a-2828-41ef-9972-bcf917d4dc56" },
      },
      { scope: "web", profiles: ["baseline"], attributes: [], tradeoffs: [] },
    ],
    findings: [{ kind: "unmeasured_high_importance", subject: "web", detail: "security.integrity has no measured scenario" }],
    catalogue: [
      cat("functional-suitability", "Functional suitability"),
      cat("reliability", "Reliability", [["recoverability", "Recoverability"]]),
      cat("security", "Security", [["integrity", "Integrity"]]),
      cat("maintainability", "Maintainability", [["modifiability", "Modifiability"]]),
      cat("safety", "Safety"),
    ],
    series: [{ id: "scrap_rate", points: [["2026-09-23", 0.2], ["2026-09-24", 0.4], ["2026-09-25", 0.5]] }],
  };
}

function fakeElements() {
  const el = (extra) => ({ innerHTML: "", textContent: "", hidden: false, querySelectorAll: () => [], ...extra });
  const ids = [
    "qa-scope-note", "qa-error", "qa-empty", "qa-body", "qa-count", "qa-view-seg", "qa-scope-pick", "qa-scope-select",
    "qa-view-heatmap", "qa-view-tree", "qa-view-tradeoffs", "qa-view-grid", "qa-view-table",
    "qa-heatmap", "qa-legend", "qa-tree", "qa-tradeoffs", "qa-grid", "qa-table", "qa-findings", "qa-no-findings",
  ];
  return Object.fromEntries(ids.map((id) => [id, el()]));
}

test("loadQuality fetches the rail's scope and draws every view from the one answer", async () => {
  const elements = fakeElements();
  globalThis.document = { ...bare, getElementById: (id) => (id in elements ? elements[id] : null) };
  const requested = [];
  globalThis.fetch = async (path) => {
    requested.push(path);
    return { status: 200, statusText: "OK", json: async () => ({ status: "ok", data: { kind: "quality", report: fixtureReport() } }) };
  };
  state.scope = "demo";

  await loadQuality();

  assert.deepEqual(requested, ["/api/quality?scope=demo"]);
  assert.equal(elements["qa-body"].hidden, false);
  assert.equal(elements["qa-empty"].hidden, true);
  assert.equal(elements["qa-count"].textContent, "2 scopes");

  const heat = elements["qa-heatmap"].innerHTML;
  assert.match(heat, /qa-st-not_met qa-imp-H/, "the maintainability cell: not met, H border");
  assert.match(heat, /qa-st-no_data qa-imp-H/);
  assert.match(heat, /web, Safety: not declared/, "an undeclared cell is labelled as such, not as a status");
  assert.equal((heat.match(/qa-hm-btn/g) || []).length, 3, "only declared cells are buttons");

  const tree = elements["qa-tree"].innerHTML;
  assert.match(tree, /When <em[^>]*>daemon restarted while 3 runs are active<\/em>/);
  assert.match(tree, /class="qa-bullet"/, "a bullet chart for a metric measure with a value");
  assert.match(tree, /class="qa-spark"/, "and its sparkline, since scrap_rate has a series");
  assert.match(tree, /data-qa-remediate/, "not_met with no open task offers Create task");
  assert.match(tree, /Task open →/, "the open remediation task is shown instead of a second Create task");
  assert.equal((tree.match(/data-qa-remediate>/g) || []).length, 1, "never for no_data, never beside an open task");

  assert.match(elements["qa-table"].innerHTML, /daemon-restart/);
  assert.match(elements["qa-grid"].innerHTML, /qa-hot/);
  assert.match(elements["qa-tradeoffs"].innerHTML, /qa-tm-btn/);
  assert.match(elements["qa-findings"].innerHTML, /H-importance attribute with no measured scenario/);
  assert.equal(elements["qa-no-findings"].hidden, true);

  delete globalThis.fetch;
});

test("no profiles bound: the empty state shows, and findings still do; a failed fetch shows the error instead", async () => {
  const elements = fakeElements();
  globalThis.document = { ...bare, getElementById: (id) => (id in elements ? elements[id] : null) };
  globalThis.fetch = async () => ({
    status: 200,
    statusText: "OK",
    json: async () => ({ status: "ok", data: { kind: "quality", report: {
      scopes: [], catalogue: [], findings: [{ kind: "missing_profile", subject: "dev", detail: "no file for `daemon`" }],
    } } }),
  });
  state.scope = null;
  await loadQuality();
  assert.equal(elements["qa-empty"].hidden, false);
  assert.equal(elements["qa-body"].hidden, true);
  assert.match(elements["qa-findings"].innerHTML, /no file for `daemon`/, "the reason there are no scopes is not hidden by the empty state");

  globalThis.fetch = async () => ({ status: 400, statusText: "Bad Request", json: async () => ({ status: "error", message: "no such scope: gone" }) });
  await loadQuality();
  assert.equal(elements["qa-error"].textContent, "no such scope: gone");
  assert.equal(elements["qa-error"].hidden, false);
  assert.equal(elements["qa-empty"].hidden, true, "an error is not also a claim that nothing is authored");

  delete globalThis.fetch;
  globalThis.document = bare;
  renderQuality(); // no throw with the bare shim
});
