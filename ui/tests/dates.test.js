import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { state } from "../js/core.js";
import { datesQuery, expiryText, stateText, dependencyWarnings, renewalInboxRows, datesSummary } from "../js/dates-model.js";
import { dateRow, dateBadges, loadDateBadges, refreshDates, renderDates } from "../js/dates.js";
import { neededEndpoints } from "../js/dashboard-tiles-model.js";
import { validateTile } from "../js/dashboard-model.js";

const entry = (patch = {}) => ({
  observation: { id: "claude", name: "Claude token", kind: "credential", scope: null, expires_at: "2026-10-20T00:00:00Z", no_expiry: false, basis: "observed", source: "openshell", detail: "expiry metadata", observed_at: "2026-10-04T00:00:00Z", issue: null, lead_seconds: 30 * 86400, owner: "owner", renew: "rotate at source", affects: [{ scope: "demo", agent: "curator", environment: null, provider: "claude", label: "demo/curator" }] },
  state: "due_soon", milestone: "lead", resolved: false, conflict: false, scheduled_risks: [], href: "#all/dates", ...patch,
});
const report = (entries = [entry()]) => ({ entries, due_soon: 1, overdue: 0, unknown: 0, next_expiry: "2026-10-20T00:00:00Z", observation_issues: [] });
const response = data => ({ json: async () => ({ status: "ok", data: { report: data } }) });

test("renewal Inbox ids stay stable at each milestone and disappear after renewal", () => {
  for (const milestone of ["lead", "seven_days", "one_day", "expired", "scheduled_run"]) {
    const rows = renewalInboxRows(report([entry({ milestone })]));
    assert.equal(rows.length, 1);
    assert.equal(rows[0].id, "renewal:claude");
    assert.match(rows[0].reason, /demo\/curator.*rotate at source.*owner/);
  }
  assert.deepEqual(renewalInboxRows(report([entry({ milestone: null, state: "ok" })])), []);
  assert.deepEqual(renewalInboxRows(report([entry({ resolved: true })])), []);
});

test("scheduled warning names its run, and native clocks never duplicate their Inbox", () => {
  const date = entry({ milestone: "scheduled_run", scheduled_risks: [{ scope: "demo", agent: "curator", title: "weekly audit", next_run_at: "2026-11-01T00:00:00Z" }] });
  assert.match(renewalInboxRows(report([date]))[0].reason, /weekly audit.*2026-11-01/);
  for (const source of ["policy_attestation", "cra_deadline"]) {
    assert.deepEqual(renewalInboxRows(report([{ ...date, observation: { ...date.observation, source } }])), []);
  }
});

test("badges use exact agent/environment dependencies, not coincident names or unknown labels", () => {
  const r = report();
  assert.equal(dependencyWarnings(r, { scope: "demo", agent: "curator" }).length, 1);
  assert.equal(dependencyWarnings(r, { scope: "other", agent: "curator" }).length, 0);
  assert.equal(dependencyWarnings(r, { scope: "demo", agent: "other" }).length, 0);
  assert.equal(dependencyWarnings(r, { scope: "demo", environment: "production" }).length, 0);
  const date = entry(); date.observation.affects = [{ scope: "demo", environment: "production", agent: null, provider: null }];
  assert.equal(dependencyWarnings(report([date]), { scope: "demo", environment: "production" }).length, 1);
  assert.equal(dependencyWarnings(report([date]), { scope: "demo", agent: "curator" }).length, 0);
  date.resolved = true;
  assert.deepEqual(dependencyWarnings(report([date]), { scope: "demo", environment: "production" }), []);
});

test("date presentation distinguishes no reported expiry, unknown and native resolution", () => {
  const date = entry(); date.observation.no_expiry = true; date.observation.source = "github_auth";
  assert.equal(expiryText(date), "no reported expiry");
  date.observation.no_expiry = false; date.observation.expires_at = null;
  assert.equal(expiryText(date), "unknown");
  assert.equal(stateText({ ...date, resolved: true, state: "overdue" }), "resolved");
  assert.equal(datesSummary(null).next, "unknown");
});

test("date rows show conflicts, last-known failures, renewal and native links, escaped", () => {
  const date = entry({ conflict: true, declared_expires_at: "2090-01-01T00:00:00Z" });
  date.observation.name = '<img src=x onerror="bad">';
  date.observation.issue = "tool unavailable";
  date.observation.source = "policy_attestation";
  const html = dateRow(date);
  assert.doesNotMatch(html, /<img/);
  assert.match(html, /&lt;img/);
  assert.match(html, /2090-01-01.*Observed metadata wins/);
  assert.match(html, /last known.*fresh observation/);
  assert.match(html, /rotate at source/);
  assert.match(html, /Open native policy \/ CRA clock/);
});

test("Important Dates is served, navigable and a valid gated dashboard tile", () => {
  const app = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");
  const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
  const assets = readFileSync(new URL("../../crates/factory-daemon/src/ui.rs", import.meta.url), "utf8");
  assert.match(page, /id="tab-dates".*Important dates/);
  assert.match(page, /id="view-dates"/);
  assert.match(app, /dates: \{ onShow: startDates, onHide: stopAgentPoll \}/);
  assert.match(app, /important_dates_updated/);
  assert.match(assets, /"js\/dates-model.js"/);
  assert.deepEqual(validateTile({ view: "important_dates", size: "m" }), []);
  assert.deepEqual(neededEndpoints([{ view: "important_dates", size: "m" }]), { metrics: false, occupancy: false, operations: false, policy: false, costs: false, dates: true });
  assert.equal(datesQuery("a/b & c"), "/api/important-dates?scope=a%2Fb+%26+c");
});

test("a stale scoped response cannot replace the new selection; unavailable never means zero", async t => {
  const host = { innerHTML: "" };
  t.mock.method(globalThis, "fetch", async () => response(null));
  globalThis.document = { getElementById: id => id === "dates" ? host : null };
  state.scope = "old";
  let finish;
  globalThis.fetch = () => new Promise(resolve => { finish = resolve; });
  const old = refreshDates();
  assert.match(host.innerHTML, /Loading important dates/);
  state.scope = "new";
  globalThis.fetch = async () => response(report([]));
  await refreshDates();
  finish(response(report())); await old;
  assert.equal(state.dates.entries.length, 0);
  state.dates = null; renderDates();
  assert.match(host.innerHTML, /not available.*No expiry state is inferred/);
  state.scope = null;
  delete globalThis.document;
});

test("all three dependency badge surfaces use the shared metadata; badges escape labels", async t => {
  const date = entry(); date.observation.name = '<script>bad</script>';
  t.mock.method(globalThis, "fetch", async () => response(report([date])));
  await loadDateBadges();
  const html = dateBadges({ scope: "demo", agent: "curator" });
  assert.match(html, /&lt;script/); assert.doesNotMatch(html, /<script/);
  for (const name of ["agents", "sandboxes", "environments"]) {
    assert.match(readFileSync(new URL(`../js/${name}.js`, import.meta.url), "utf8"), /dateBadges\(/);
  }
  globalThis.fetch = async () => response(null);
  await loadDateBadges();
  assert.match(dateBadges({ scope: "demo", agent: "curator" }), /dates unavailable/);
});
