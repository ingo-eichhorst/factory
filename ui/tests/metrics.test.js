import test from "node:test";
import assert from "node:assert/strict";

import { state } from "../js/core.js";
import { loadMetrics } from "../js/metrics.js";

const IDS = ["metrics-note", "metrics-error", "metrics-rows", "metrics-empty", "metrics-count", "metrics-findings"];

function fakeDom() {
  const elements = Object.fromEntries(
    IDS.map((id) => [id, { textContent: "", hidden: false, innerHTML: "", querySelectorAll: () => [] }]),
  );
  globalThis.document = { getElementById: (id) => elements[id] || null };
  return elements;
}

function answer(data) {
  return async (path) => {
    answer.asked = path;
    return { status: 200, statusText: "OK", json: async () => ({ status: "ok", data }) };
  };
}

test("loadMetrics asks for the selected scope's listing and draws one row per metric, with source and consumers", async () => {
  const elements = fakeDom();
  globalThis.fetch = answer({
    kind: "metrics",
    values: [{ id: "reported.finance.bookings_without_beleg", value: 2, as_of: "2026-10-20T09:00:00Z" }],
    registry: [{ id: "reported.finance.bookings_without_beleg", title: "Bookings without Beleg", unit: "count", better: "lower" }],
    rows: [
      {
        id: "reported.finance.bookings_without_beleg",
        source: { kind: "service", source: "finance", scope: "finance", service: "fava", endpoint: "127.0.0.1:8793", path: "/m", url: "http://127.0.0.1:8793/m" },
        freshness: "fresh",
        max_age: "1w",
        consumers: [{ kind: "policy", id: "gobd/belegprinzip", scope: "finance", status: "not_met", below: 0 }],
      },
    ],
    findings: [{ kind: "no_loopback_endpoint", subject: "other/src", detail: "no loopback endpoint" }],
  });
  try {
    state.scope = "finance";
    await loadMetrics();
    assert.equal(answer.asked, "/api/metrics?scope=finance");
    const html = elements["metrics-rows"].innerHTML;
    assert.match(html, /Bookings without Beleg/);
    assert.match(html, /service · fava/);
    assert.match(html, /data-dep-service="fava"/);
    assert.match(html, /control gobd\/belegprinzip @ finance/);
    assert.match(html, /s-not_met/);
    assert.equal(elements["metrics-count"].textContent, "1 metric");
    assert.equal(elements["metrics-empty"].hidden, true);
    assert.equal(elements["metrics-error"].hidden, true);
    assert.match(elements["metrics-findings"].innerHTML, /no loopback endpoint/);
  } finally {
    delete globalThis.fetch;
    delete globalThis.document;
    state.scope = null;
  }
});

test("with every scope selected and no root scope, nothing is asked and the note says to select one", async () => {
  const elements = fakeDom();
  let asked = false;
  globalThis.fetch = async () => { asked = true; return { json: async () => null }; };
  try {
    state.scope = null;
    state.scopes = [{ name: "finance", path: "/inst/projects/finance" }];
    state.root = "/inst";
    await loadMetrics();
    assert.equal(asked, false);
    assert.match(elements["metrics-note"].textContent, /Select one scope/);
    assert.equal(elements["metrics-rows"].innerHTML, "");
  } finally {
    delete globalThis.fetch;
    delete globalThis.document;
    state.scopes = [];
    state.root = "";
  }
});

test("a failed read shows the daemon's message and draws nothing", async () => {
  const elements = fakeDom();
  globalThis.fetch = async () => ({ status: 404, statusText: "Not Found", json: async () => ({ status: "error", message: "no such scope: gone" }) });
  try {
    state.scope = "gone";
    await loadMetrics();
    assert.equal(elements["metrics-error"].hidden, false);
    assert.match(elements["metrics-error"].textContent, /no such scope/);
    assert.equal(elements["metrics-rows"].innerHTML, "");
    assert.equal(elements["metrics-empty"].hidden, true);
  } finally {
    delete globalThis.fetch;
    delete globalThis.document;
    state.scope = null;
  }
});
