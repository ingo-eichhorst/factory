import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  CONFIG_SNIPPET,
  MISSING,
  actorText,
  bucketTone,
  budgetLevel,
  budgetText,
  deployTone,
  deploymentRows,
  doraRows,
  durationText,
  environmentsFailure,
  environmentsQuery,
  fmtPct,
  incidentText,
  isEnvironmentsEvent,
  lastCheckText,
  releaseRows,
  releaseText,
  sloText,
  statusText,
  statusTone,
  uptimeLevel,
  verificationText,
} from "../js/environments-model.js";
import { state } from "../js/core.js";
import { readHash, setRouter } from "../js/scopes.js";

const bare = { addEventListener() {}, getElementById: () => null };
globalThis.document = bare;
const { loadEnvironments, renderEnvironments } = await import("../js/environments.js");

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const app = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");
const view = readFileSync(new URL("../js/environments.js", import.meta.url), "utf8");
const served = readFileSync(new URL("../../crates/factory-daemon/src/ui.rs", import.meta.url), "utf8");
const REPORT = JSON.parse(readFileSync(new URL("./fixtures/environments-report.json", import.meta.url), "utf8"));
const NOW = REPORT.generated_at;
const [REVIEW, STAGING, PRODUCTION] = REPORT.environments;

// ------------------------------------------------------------- the frame

test("#185: L1 gains an Operations tab between Infrastructure and Backup, and it is served", () => {
  assert.match(page, /id="tab-infrastructure"[^>]*>Infrastructure<\/button>\s*<button id="tab-environments" hidden>Operations<\/button>\s*<button id="tab-backup"/);
  assert.match(page, /<div id="view-environments" hidden>\s*<div class="bar">\s*<h2>Operations<\/h2>/);
  assert.match(page, /id="environments-unavailable"/);
  assert.match(page, /id="environments-error"/);
  assert.match(page, /<span class="lv-sub">Host, daemon, AI accounts, operations and backup<\/span>/);
  assert.match(app, /infra: \["infrastructure", "environments", "backup"\]/);
  assert.match(app, /environments: \{ onShow: startEnvironments, onHide: stopAgentPoll \}/);
  assert.match(app, /state\.agentPoll = setInterval\(refreshEnvironments, 30000\)/);
  assert.match(app, /if \(isEnvironmentsEvent\(ev\) && state\.tab === "environments"\) refreshEnvironments\(\);/);
  assert.match(app, /if \(state\.tab === "environments"\) \{ refreshEnvironments\(\); return; \}/, "a rail change refetches");
  assert.match(served, /"js\/environments\.js"/);
  assert.match(served, /"js\/environments-model\.js"/);
  assert.match(view, /\/api\/environments/);
  assert.doesNotMatch(view, /(?<![.\w])(alert|confirm|prompt)\(/);
});

test("an old link to the L4 Operations tab lands on Line, levelled or bare", () => {
  setRouter({ pages: ["tasks", "line", "environments"], redirects: { operations: (tail) => ({ page: "line", tail }) } });
  globalThis.location = { hash: "#factory/proc/operations/task/t1" };
  let route = readHash();
  assert.equal(route.page, "line");
  assert.equal(route.scope, "factory");
  assert.deepEqual(route.tail, ["task", "t1"]);
  globalThis.location = { hash: "#all/operations" };
  route = readHash();
  assert.equal(route.page, "line");
  assert.equal(route.scope, null);
  delete globalThis.location;
});

// ------------------------------------------------------------- the model

test("status reads as a colour and a word, with how long", () => {
  assert.equal(statusTone("up"), "ok");
  assert.equal(statusTone("degraded"), "warn");
  assert.equal(statusTone("down"), "bad");
  assert.equal(statusTone("unknown"), "none");
  assert.equal(statusText(STAGING, NOW), "Down for 10m");
  assert.equal(statusText(PRODUCTION, NOW), "Up for 2d 0h");
  assert.equal(statusText(REVIEW, NOW), "Unknown");
  assert.equal(statusText({ ...PRODUCTION, paused: true, status: "unknown" }, NOW), "Unknown · checks paused");
});

test("a missing figure is missing, never 0% and never 100%", () => {
  assert.equal(fmtPct(null), MISSING);
  assert.equal(fmtPct(undefined), MISSING);
  assert.equal(fmtPct(1), "100%");
  assert.equal(fmtPct(0), "0%");
  assert.equal(fmtPct(0.995), "99.50%");
  assert.equal(fmtPct(0.97), "97.0%");
  assert.equal(uptimeLevel(null, STAGING.slo), "none");
  assert.equal(uptimeLevel(0.97, null), "none", "no target, nothing to judge");
  assert.equal(uptimeLevel(0.97, STAGING.slo), "bad");
  assert.equal(uptimeLevel(0.995, STAGING.slo), "ok");
  assert.equal(budgetText(null), MISSING);
  assert.equal(budgetLevel(null), "none");
  assert.equal(budgetText(-0.5), "overspent by 50.0%");
  assert.equal(budgetLevel(-0.5), "bad");
  assert.equal(budgetLevel(0.1), "warn");
  assert.equal(budgetText(0.6), "60.0% left");
  assert.equal(budgetLevel(0.6), "ok");
  assert.equal(sloText(null), "no SLO declared");
  assert.equal(sloText(PRODUCTION.slo), "99.50% over 28d");
  const dora = doraRows(PRODUCTION.dora);
  assert.deepEqual(dora.map(r => r.value), ["0.5", "2h 0m", "50.0%", MISSING, MISSING]);
  assert.deepEqual(doraRows(undefined).map(r => r.value), [MISSING, MISSING, MISSING, MISSING, MISSING]);
});

test("the timeline keeps the report's order, and failures stand out", () => {
  const rows = deploymentRows(REPORT);
  assert.deepEqual(rows.map(r => r.id), ["d5", "d4", "d1"], "running first, then newest -- the daemon's order");
  const [running, failed, rolled] = rows;
  assert.equal(running.status, "running");
  assert.equal(running.tone, "warn");
  assert.equal(running.duration, "2m so far");
  assert.equal(running.who, "builder · run 1a2b3c4d");
  assert.equal(running.verification.text, MISSING);
  assert.ok(failed.standsOut && rolled.standsOut && !running.standsOut);
  assert.equal(failed.tone, "bad");
  assert.equal(failed.duration, "3m");
  assert.equal(failed.who, "Codex Builder (agent)");
  assert.equal(failed.verification.level, "bad");
  assert.match(failed.verification.text, /FAILED · \/api\/status: expected 200, got 502/);
  assert.equal(rolled.status, "rolled back");
  assert.equal(rolled.who, "owner (by hand)");
  assert.equal(rolled.release, "aaaaaaaaaa (dirty)");
  assert.equal(rolled.verification.text, "not verified");
  assert.equal(deployTone("succeeded"), "ok");
  assert.equal(actorText(PRODUCTION.current), "owner via release.sh");
  assert.deepEqual(verificationText(PRODUCTION.current), { level: "ok", text: "passed · 1 check" });
  assert.equal(durationText(PRODUCTION.current, NOW), "5m");
});

test("the catalogue says where each release runs and how its deployments went", () => {
  const rows = releaseRows(REPORT);
  assert.equal(rows.length, 3);
  assert.deepEqual(rows[0].runningOn, [], "a release not running anywhere is still listed");
  assert.deepEqual(rows[2].runningOn, ["production"]);
  assert.equal(rows[2].name, "v0.2");
  assert.equal(rows[2].failed, 1);
  assert.equal(releaseText(PRODUCTION.current.release), "v0.2 · bbbbbbbbbb");
  assert.equal(releaseText(REVIEW.current.release), "v0.3-2-gccccccc · cccccccccc");
  assert.equal(releaseText(null), MISSING);
});

test("a check's strip and newest answer, and incidents, in words", () => {
  const check = STAGING.checks[0];
  assert.deepEqual(check.strip.map(bucketTone), ["ok", "bad", "none"]);
  assert.equal(lastCheckText(check, NOW), "failing · timed out after 5s · 5000ms · 30s ago");
  assert.equal(lastCheckText({ last: null }, NOW), "not checked yet");
  assert.equal(incidentText(STAGING.incidents[0], NOW), "2026-09-26 11:50 UTC · open for 10m · /api/status");
  assert.equal(incidentText(STAGING.incidents[1], NOW), "2026-09-20 08:00 UTC · lasted 10m · /api/status");
});

test("events, the scope query and a daemon without the endpoint", () => {
  assert.ok(isEnvironmentsEvent({ type: "deployment_updated", deployment: {} }));
  assert.ok(isEnvironmentsEvent({ type: "environment_status_changed", environment: "staging", status: "down" }));
  assert.ok(!isEnvironmentsEvent({ type: "backup_completed" }));
  assert.ok(!isEnvironmentsEvent(null));
  assert.equal(environmentsQuery(null), "");
  assert.equal(environmentsQuery("projects/demo"), "?scope=projects%2Fdemo");
  assert.equal(environmentsFailure(new Error("404 Not Found")), "unavailable");
  assert.equal(environmentsFailure(new Error("500 boom")), "error");
  assert.match(CONFIG_SNIPPET, /environments:/);
  assert.match(CONFIG_SNIPPET, /promotes_to: production/);
});

// ------------------------------------------------------------- rendering

function stubPage(ids) {
  const elements = {};
  for (const id of ids) elements[id] = { innerHTML: "", textContent: "", hidden: true, disabled: false };
  globalThis.document = { ...bare, getElementById: (id) => elements[id] || null };
  return elements;
}

const IDS = ["environments", "environments-error", "environments-unavailable", "environments-generated", "environments-refresh"];

function answering(body, requested, status = 200) {
  return async (path) => {
    requested.push(path);
    return {
      ok: status < 400,
      status,
      statusText: status === 404 ? "Not Found" : "OK",
      // A daemon that predates the route answers axum's bare 404: no JSON.
      json: async () => {
        if (status >= 400) throw new SyntaxError("not JSON");
        return { status: "ok", data: { kind: "environments", report: body } };
      },
      text: async () => "",
    };
  };
}

test("the page draws cards in the report's order, the timeline and the catalogue, from one scoped read", async () => {
  const el = stubPage(IDS);
  const requested = [];
  globalThis.fetch = answering(REPORT, requested);
  state.scope = "factory";
  await loadEnvironments();
  renderEnvironments();
  assert.deepEqual(requested, ["/api/environments?scope=factory"]);
  const html = el.environments.innerHTML;
  assert.ok(!el.environments.hidden);
  const order = ["review13", "staging", "production"].map(n => html.indexOf(`id="sys-${n}"`));
  assert.ok(order.every(i => i >= 0) && order[0] < order[1] && order[1] < order[2], "promotion order");
  for (const words of ["Deployments", "Releases", "Deploying", "overspent by 50.0%", "Not declared in any scope", "rolled back", "Lead time p50"]) {
    assert.ok(html.includes(words), words);
  }
  assert.equal((html.match(/class="sys-slot"/g) || []).length, 3);
  assert.match(html, /class="sys-failed"/);
  assert.match(el["environments-generated"].textContent, /^as of 12:00:00 UTC$/);
  state.scope = null;
});

test("nothing declared shows the config to write; an old daemon is a calm note, not an error", async () => {
  let el = stubPage(IDS);
  globalThis.fetch = answering({ ...REPORT, environments: [], deployments: [], releases: [] }, []);
  await loadEnvironments();
  renderEnvironments();
  assert.match(el.environments.innerHTML, /No environments declared/);
  assert.match(el.environments.innerHTML, /promotes_to: production/);
  assert.match(el.environments.innerHTML, /No deployment recorded yet/);

  el = stubPage(IDS);
  globalThis.fetch = answering(null, [], 404);
  await loadEnvironments();
  renderEnvironments();
  assert.equal(el["environments-unavailable"].hidden, false);
  assert.equal(el["environments-error"].hidden, true);
  assert.equal(el.environments.hidden, true);
});
