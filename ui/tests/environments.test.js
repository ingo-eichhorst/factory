import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  CONFIG_SNIPPET,
  MISSING,
  actorText,
  bucketTone,
  bucketHasIncident,
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
  promotionChoices,
  recoveryStatus,
  samplesSelection,
  samplesQuery,
  releaseRows,
  releaseEffectiveness,
  releaseDetailQuery,
  releaseRepositoryURL,
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
const { loadEnvironments, renderEnvironments, promoteEnvironment, recoverEnvironment, publishDeployment, loadEnvironmentSamples, loadReleaseDetail, wireEnvironments } = await import("../js/environments.js");

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const app = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");
const view = readFileSync(new URL("../js/environments.js", import.meta.url), "utf8");
const served = readFileSync(new URL("../../crates/factory-daemon/src/ui.rs", import.meta.url), "utf8");
const REPORT = JSON.parse(readFileSync(new URL("./fixtures/environments-report.json", import.meta.url), "utf8"));
const NOW = REPORT.generated_at;
const [REVIEW, STAGING, PRODUCTION] = REPORT.environments;

// ------------------------------------------------------------- the frame

test("#185: L1 gains an Operations tab between Infrastructure and Backup, and it is served", () => {
  assert.match(page, /id="tab-infrastructure"[^>]*>Infrastructure<\/button>\s*<button id="tab-doctor" hidden>Doctor<\/button>\s*<button id="tab-environments" hidden>Operations<\/button>\s*<button id="tab-backup"/);
  assert.match(page, /<div id="view-environments" hidden>\s*<div class="bar">\s*<h2>Operations<\/h2>/);
  assert.match(page, /id="environments-unavailable"/);
  assert.match(page, /id="environments-error"/);
  assert.match(page, /<span class="lv-sub">Host, daemon, Doctor, operations and backup<\/span>/);
  assert.match(app, /infra: \["infrastructure", "doctor", "environments", "backup"\]/);
  assert.match(app, /environments: \{ onShow: startEnvironments, onHide: stopAgentPoll \}/);
  assert.match(app, /state\.agentPoll = setInterval\(refreshEnvironments, 30000\)/);
  assert.match(app, /if \(isEnvironmentsEvent\(ev\) && state\.tab === "environments"\) refreshEnvironments\(\)\.catch\(/);
  assert.match(app, /if \(state\.tab === "environments"\) \{ refreshEnvironments\(\)\.catch\([\s\S]*?return; \}/, "a rail change refetches");
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

test("slow successful checks are amber and retain the recorded reason", () => {
  assert.equal(bucketTone({ ok: 3, failed: 0, slow: 1 }), "warn");
  assert.equal(bucketTone({ ok: 3, failed: 1, slow: 1 }), "bad");
  assert.equal(bucketTone({ ok: 3, failed: 0 }), "ok", "old daemon still works");
  const last = { ok: true, slow: true, detail: "200; slow: 1000ms exceeds 750ms", latency_ms: 1000, at: STAGING.checks[0].last.at };
  assert.equal(lastCheckText({ last }, NOW), "slow · 200; slow: 1000ms exceeds 750ms · 1000ms · 30s ago");
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

test("the rendered card and strip show recorded slowness and the threshold", () => {
  const el = stubPage(IDS);
  const check = STAGING.checks[0];
  state.environments = {
    ...REPORT,
    environments: [{
      ...STAGING, status: "degraded",
      checks: [{ ...check, slow_after_ms: 750, last: { ...check.last, ok: true, slow: true, detail: "200; slow: 1000ms exceeds 750ms" }, strip: [{ start: NOW, ok: 1, failed: 0, slow: 1 }] }],
    }],
  };
  renderEnvironments();
  const html = el.environments.innerHTML;
  assert.match(html, /class="sys-dot" data-tone="warn"/);
  assert.match(html, /class="sys-slot" data-tone="warn"/);
  assert.match(html, /1 ok \(1 slow\), 0 failed/);
  assert.match(html, /slow above 750ms/);
  assert.match(html, /200; slow: 1000ms exceeds 750ms/);
  const css = readFileSync(new URL("../app.css", import.meta.url), "utf8");
  assert.match(css, /\.sys-slot\[data-tone="warn"\]\s*\{\s*background: var\(--wait\)/);
});

function promotableReport() {
  const report = structuredClone(REPORT);
  report.environments[1].promotion_ready = true;
  report.environments[1].current = { ...report.environments[2].current, id: "verified-source" };
  return report;
}

test("catalogue promotion choices require daemon readiness and the exact source scope/commit", () => {
  const report = promotableReport();
  const release = report.releases.find(r => r.commit === report.environments[1].current.release.commit);
  assert.deepEqual(promotionChoices(report, release), [{ source: "staging", target: "production", deployment: "verified-source" }]);
  assert.deepEqual(promotionChoices(REPORT, release), [], "older daemon exposes no actionable promotion");
  assert.deepEqual(promotionChoices(report, { ...release, scope: "elsewhere" }), []);
  assert.deepEqual(promotionChoices(report, { ...release, commit: "another" }), []);
  report.environments[1].promotion_ready = false;
  assert.deepEqual(promotionChoices(report, release), []);
});

test("promotion posts a frozen deployment selection once and links the pending approval workflow", async () => {
  const el = stubPage(IDS);
  const report = promotableReport();
  state.environments = report;
  state.scope = "factory";
  state.environmentsError = null;
  state.environmentsUnavailable = false;
  renderEnvironments();
  wireEnvironments();
  assert.equal(typeof el.environments.onclick, "function");
  assert.equal((el.environments.innerHTML.match(/data-environment-promote="staging"/g) || []).length, 2, "card and catalogue");
  const calls = [];
  let finish;
  globalThis.fetch = async (path, options) => {
    calls.push({ path, options });
    if (options?.method === "POST") {
      await new Promise(resolve => { finish = resolve; });
      return { ok: true, json: async () => ({ status: "ok", data: { kind: "workflow_run", run: { scope: "factory", workflow_id: "promotion", id: "approval" } } }) };
    }
    return { ok: true, json: async () => ({ status: "ok", data: { kind: "environments", report } }) };
  };
  const pending = promoteEnvironment("staging", "verified-source");
  assert.match(el.environments.innerHTML, /data-deployment="verified-source" disabled/);
  await promoteEnvironment("staging", "verified-source");
  await promoteEnvironment("staging", "stale");
  assert.equal(calls.length, 1, "duplicate or stale clicks cannot submit another workflow");
  assert.equal(calls[0].path, "/api/environments/promote");
  assert.deepEqual(JSON.parse(calls[0].options.body), { environment: "staging", deployment: "verified-source" });
  finish();
  await pending;
  assert.equal(calls[1].path, "/api/environments?scope=factory");
  assert.match(el.environments.innerHTML, /#factory\/proc\/workflows\/promotion\/run\/approval/);
  assert.match(el.environments.innerHTML, /Deployment waits for owner approval/);
  state.scope = "another";
  renderEnvironments();
  assert.doesNotMatch(el.environments.innerHTML, /Promotion created/);
  state.scope = null;
});

test("a refused promotion shows an escaped reason and does not claim success", async () => {
  const el = stubPage(IDS);
  state.environments = promotableReport();
  state.scope = "refused-promotion";
  globalThis.fetch = async (path, options) => ({ ok: true, json: async () => options?.method === "POST"
    ? { status: "error", message: "source changed <refresh>" }
    : { status: "ok", data: { kind: "environments", report: promotableReport() } } });
  await promoteEnvironment("staging", "verified-source");
  assert.match(el.environments.innerHTML, /source changed &lt;refresh&gt;/);
  assert.doesNotMatch(el.environments.innerHTML, /Promotion created/);
  state.scope = null;
});

test("sample selections cap the current bucket, encode names and mark only overlapping incidents", () => {
  const report = { generated_at: "2026-10-04T12:05:00Z" };
  const selection = samplesSelection(report, "staging", "/api/status", "2026-10-04T12:00:00Z");
  assert.equal(selection.to, "2026-10-04T12:05:00.000Z", "current slot never asks for a future window");
  const url = samplesQuery(selection, "projects/demo", 17);
  assert.match(url, /check=%2Fapi%2Fstatus/);
  assert.match(url, /scope=projects%2Fdemo/);
  assert.match(url, /before=17/);
  const incident = { checks: ["api"], started_at: "2026-10-04T12:03:00Z", ended_at: "2026-10-04T12:07:00Z" };
  assert.equal(bucketHasIncident({ start: selection.from }, { name: "api" }, [incident]), true);
  assert.equal(bucketHasIncident({ start: selection.from }, { name: "disk" }, [incident]), false);
  assert.equal(bucketHasIncident({ start: "2026-10-04T12:30:00Z" }, { name: "api" }, [incident]), false);
  assert.equal(recoveryStatus({ status: "running", run: { status: "failed" } }), "failed", "the reported run failure is not hidden by its open workflow");
});

test("sample drill-down pages the same fixed window and renders answers, latency and escaped details", async () => {
  const el = stubPage(IDS);
  state.environments = structuredClone(REPORT);
  state.scope = "sample-scope";
  state.environmentsError = null;
  state.environmentsUnavailable = false;
  const requested = [];
  const pages = [
    { next_before: 4, samples: [{ id: 5, at: NOW, ok: true, slow: true, latency_ms: 1000, detail: "slow <reason>" }] },
    { samples: [{ id: 3, at: NOW, ok: false, slow: false, latency_ms: 5, detail: "failed earlier" }] },
  ];
  globalThis.fetch = async path => {
    requested.push(path);
    return { ok: true, json: async () => ({ status: "ok", data: { kind: "environment_samples", page: pages.shift() } }) };
  };
  const check = STAGING.checks[0].name;
  await loadEnvironmentSamples("staging", check);
  assert.match(el.environments.innerHTML, /Health samples: staging/);
  assert.match(el.environments.innerHTML, /slow &lt;reason&gt;/);
  assert.match(el.environments.innerHTML, /1000ms/);
  assert.match(el.environments.innerHTML, /Older samples/);
  await loadEnvironmentSamples("staging", check, null, 4);
  const first = new URL(requested[0], "https://test.invalid").searchParams;
  const second = new URL(requested[1], "https://test.invalid").searchParams;
  assert.equal(second.get("from"), first.get("from"));
  assert.equal(second.get("to"), first.get("to"));
  assert.equal(second.get("before"), "4");
  assert.match(el.environments.innerHTML, /failed earlier/);
  state.scope = "another-scope";
  renderEnvironments();
  assert.doesNotMatch(el.environments.innerHTML, /Health samples:/);
  state.scope = null;
});

test("stale sample responses cannot replace a newer selection or leak across scopes", async () => {
  const el = stubPage(IDS);
  state.environments = { ...REPORT, environments: [STAGING, { ...PRODUCTION, checks: STAGING.checks }] };
  state.scope = "stale-samples";
  const finishes = [];
  globalThis.fetch = async () => {
    const page = await new Promise(resolve => finishes.push(resolve));
    return { ok: true, json: async () => ({ status: "ok", data: { page } }) };
  };
  const first = loadEnvironmentSamples("staging", STAGING.checks[0].name);
  const second = loadEnvironmentSamples("production", STAGING.checks[0].name);
  finishes[1]({ samples: [{ at: NOW, ok: true, slow: false, latency_ms: 1, detail: "new selection" }] });
  await second;
  finishes[0]({ samples: [{ at: NOW, ok: false, slow: false, latency_ms: 1, detail: "stale selection" }] });
  await first;
  assert.match(el.environments.innerHTML, /new selection/);
  assert.doesNotMatch(el.environments.innerHTML, /stale selection/);
  state.scope = "elsewhere";
  renderEnvironments();
  assert.doesNotMatch(el.environments.innerHTML, /new selection/);
  state.scope = null;
});

test("recovery requires a reason, creates a workflow and leaves deployment history separate", async () => {
  const el = stubPage(IDS);
  const report = structuredClone(REPORT);
  report.environments[1].recovery_ready = true;
  report.recoveries = [{ scope: "factory", environment: "staging", workflow_id: "repair", workflow_run_id: "attempt", status: "running",
    run: { status: "failed" }, reason: "repair <installed>", requested_at: NOW, requested_by: "owner", expected_commit: "a".repeat(40) }];
  state.environments = report;
  state.scope = "recovery-scope";
  const calls = [];
  globalThis.fetch = async (path, options) => {
    calls.push({ path, options });
    return { ok: true, json: async () => ({ status: "ok", data: options?.method === "POST"
      ? { run: { scope: "factory", workflow_id: "repair", id: "new-attempt" } } : { report } }) };
  };
  await recoverEnvironment("staging", " ");
  assert.equal(calls.length, 0);
  await recoverEnvironment("staging", "restart installed system");
  assert.equal(calls[0].path, "/api/environments/recover");
  assert.deepEqual(JSON.parse(calls[0].options.body), { environment: "staging", reason: "restart installed system" });
  assert.match(el.environments.innerHTML, /Recovery created/);
  assert.match(el.environments.innerHTML, /Command waits for owner approval/);
  assert.match(el.environments.innerHTML, /Recovery actions/);
  assert.match(el.environments.innerHTML, /repair &lt;installed&gt;/);
  assert.match(el.environments.innerHTML, /restarts and repairs, not deployments/);
  assert.match(el.environments.innerHTML, /data-environment-recover="staging"/);
  state.scope = null;
});

test("standalone script receipts show reported outcomes without inventing runs, verification or deployments", () => {
  const el = stubPage(IDS);
  state.scope = null;
  const report = structuredClone(REPORT);
  const action = { id: "receipt", scope: "factory", environment: "production", source: "ensure.sh", actor: "operator",
    reason: "restart <installed>", command: "repair <routes>", started_at: REPORT.generated_at };
  report.recovery_journal = { actions: [action], findings: ["rejected <bad> receipt"] };
  state.environments = report;
  renderEnvironments();
  assert.match(el.environments.innerHTML, /Standalone recovery journal/);
  assert.match(el.environments.innerHTML, /awaiting finish receipt; not a runtime status/);
  assert.match(el.environments.innerHTML, /LAN: not recorded/);
  assert.match(el.environments.innerHTML, /restart &lt;installed&gt;/);
  assert.match(el.environments.innerHTML, /rejected &lt;bad&gt; receipt/);
  assert.doesNotMatch(el.environments.innerHTML, /proc\/workflows\/undefined/);
  action.finish = { exit_code: 1, local_http: true, network_routes: false, detail: "route <failed>" };
  renderEnvironments();
  assert.match(el.environments.innerHTML, /reported action exit 1/);
  assert.match(el.environments.innerHTML, /LAN: passed/);
  assert.match(el.environments.innerHTML, /Required routes: failed/);
  assert.match(el.environments.innerHTML, /route &lt;failed&gt;/);
  assert.match(el.environments.innerHTML, /not Factory runs or deployments/);
  assert.match(el.environments.innerHTML, /not declared health samples or SLA evidence/);
  assert.equal(report.deployments.length, REPORT.deployments.length);
});

test("GitHub mirrors are opt-in, explicitly reviewed and become stale when approved metadata changes", async () => {
  const el = stubPage(IDS);
  state.scope = null;
  const report = structuredClone(REPORT);
  const id = report.deployments[0].id;
  const plan = { deployment: id, scope: "factory", repository: "owner/repo", commit: "full-sha", environment: "prod", state: "in_progress", approval: "frozen-digest", verified: null };
  report.deployment_mirrors = { [id]: { plan, receipt: null } };
  state.environments = report;
  renderEnvironments();
  assert.match(el.environments.innerHTML, /Awaiting explicit approval/);
  assert.match(el.environments.innerHTML, /Review mirror plan/);
  assert.match(el.environments.innerHTML, /https:\/\/github.com\/owner\/repo\/deployments/);
  report.deployment_mirrors[id].receipt = { phase: "published", plan: structuredClone(plan) };
  renderEnvironments();
  assert.doesNotMatch(el.environments.innerHTML, /Review mirror plan/);
  report.deployment_mirrors[id].plan.approval = "new-status-digest";
  renderEnvironments();
  assert.match(el.environments.innerHTML, /Review mirror plan/);
  const calls = [];
  globalThis.fetch = async (url, options) => {
    calls.push([url, options]);
    return url.endsWith("/publish")
      ? { ok: true, json: async () => ({ status: "ok", data: { receipt: { phase: "published" } } }) }
      : { ok: true, json: async () => ({ status: "ok", data: { report } }) };
  };
  await publishDeployment(id, "exact-reviewed-digest");
  assert.equal(calls[0][0], `/api/deployments/${encodeURIComponent(id)}/publish`);
  assert.equal(calls[0][1].method, "POST");
  assert.deepEqual(JSON.parse(calls[0][1].body), { approval: "exact-reviewed-digest" });
  assert.match(view, /Repository integrations may react to those events/);
  assert.match(view, /Approve this exact outbound write/);
  delete globalThis.fetch;
});

test("reviewing a GitHub mirror writes nothing, and modal approval remains frozen across refreshes", async () => {
  const el = stubPage([...IDS, "sys-mirror-close", "sys-mirror-form"]);
  const report = structuredClone(REPORT);
  const id = report.deployments[0].id;
  report.deployment_mirrors = { [id]: { plan: { deployment: id, repository: "owner/repo",
    commit: "a".repeat(40), environment: "prod", state: "success", verified: true,
    approval: "reviewed-digest" }, receipt: null } };
  state.environments = report;
  state.scope = null;
  let modal;
  document.querySelectorAll = () => [];
  document.createElement = () => ({});
  document.body = { appendChild: element => { modal = element; } };
  globalThis.location = { hash: "#all/tasks" };
  const calls = [];
  globalThis.fetch = async (url, options) => {
    calls.push([url, options]);
    return { ok: true, json: async () => ({ status: "ok", data: url.endsWith("/publish")
      ? { receipt: { phase: "published" } } : { report } }) };
  };
  wireEnvironments();
  el.environments.onclick({ target: { closest: selector => selector === "[data-deployment-publish]"
    ? { dataset: { deploymentPublish: id } } : null } });
  assert.equal(calls.length, 0, "opening the review modal cannot publish");
  assert.match(modal.innerHTML, /owner\/repo/);
  assert.match(modal.innerHTML, /Approve this exact outbound write/);
  report.deployment_mirrors[id].plan.approval = "changed-after-review";
  report.deployment_mirrors[id].plan.repository = "different/repo";
  let prevented = false;
  el["sys-mirror-form"].onsubmit({ preventDefault: () => { prevented = true; } });
  assert.ok(prevented);
  assert.deepEqual(JSON.parse(calls[0][1].body), { approval: "reviewed-digest" });
  assert.equal(calls[0][0], `/api/deployments/${encodeURIComponent(id)}/publish`);
  await new Promise(resolve => setImmediate(resolve));
  delete globalThis.fetch;
  delete globalThis.location;
});

test("health strips are selectable with incident markers, and missing samples or read failures are explicit", async () => {
  const el = stubPage(IDS);
  const report = structuredClone(REPORT);
  report.environments[1].incidents = [{ started_at: STAGING.checks[0].strip[1].start, checks: [STAGING.checks[0].name] }];
  state.environments = report;
  state.scope = "health-detail";
  renderEnvironments();
  assert.match(el.environments.innerHTML, /<button type="button" class="sys-slot"[^>]*data-incident="true"[^>]*data-samples-environment="staging"/);
  assert.match(el.environments.innerHTML, /role="group"[^>]*select a slot to view samples/);
  globalThis.fetch = async () => ({ ok: true, json: async () => ({ status: "ok", data: { page: { samples: [] } } }) });
  await loadEnvironmentSamples("staging", STAGING.checks[0].name);
  assert.match(el.environments.innerHTML, /No stored samples in this window/);
  assert.doesNotMatch(el.environments.innerHTML, /data-samples-older/);
  globalThis.fetch = async () => { throw new Error("sample read <failed>"); };
  await loadEnvironmentSamples("staging", STAGING.checks[0].name);
  assert.match(el.environments.innerHTML, /sample read &lt;failed&gt;/);
  wireEnvironments();
  el.environments.onclick({ target: { closest: selector => selector === "[data-samples-close]" ? {} : null } });
  assert.doesNotMatch(el.environments.innerHTML, /sample read &lt;failed&gt;/);
  state.scope = null;
});

test("release evidence queries keep exact scope/commit/deployment, repository links are safe and unknown CFR stays unknown", () => {
  assert.equal(releaseDetailQuery("projects/demo", "a b", "deploy/1"), "/api/releases/detail?scope=projects%2Fdemo&commit=a+b&deployment=deploy%2F1");
  assert.equal(releaseRepositoryURL({ repository: "owner/repo" }, "pull/90"), "https://github.com/owner/repo/pull/90");
  for (const repository of ["javascript:evil", "../..", "owner/repo/extra", "owner@evil/repo", "owner/repo?evil"]) {
    assert.equal(releaseRepositoryURL({ repository }, "pull/90"), null);
  }
  assert.equal(releaseEffectiveness({}), MISSING);
  assert.equal(releaseEffectiveness({ effectiveness: { change_failure_rate: null } }), MISSING);
  assert.equal(releaseEffectiveness({ effectiveness: { change_failure_rate: 0.5, failed_changes: 1, finished: 2, window_days: 28, observing: 1 } }), "50.0% · 1/2 changes · 28d · 1 still observing");
});

function detailFixture() {
  return {
    release: { ...REPORT.releases[2], commit: "b".repeat(40), scope: "factory" },
    changes: { base: "a".repeat(40), commit: "b".repeat(40), repository: "owner/repo", truncated: true,
      commits: [{ commit: "b".repeat(40), subject: "release <unsafe>", pull_requests: [90], issues: [91], references: [92] }] },
    build: { scope: "factory", task_id: "build/task", run: { id: "build/run", status: "done" }, attestations: [{ id: "a" }],
      artifacts: [{ artifact: { name: "product <unsafe>", sha256: "c".repeat(64), size_bytes: 12 } }] },
    sboms: [{ scope: "factory", commit: "b".repeat(40), version: "v1", attachment: { id: "sbom/id", filename: "build.cdx.json", attached_at: NOW } }],
  };
}

test("release details render captured changes, real build evidence, SBOM links and weekly effectiveness", async () => {
  const el = stubPage(IDS);
  const report = structuredClone(REPORT);
  report.releases[2].commit = "b".repeat(40);
  report.environments[1].effectiveness = [{ from: "2026-09-19T12:00:00Z", to: NOW, dora: { deploy_frequency: 1, lead_time_p50: 60,
    change_failure_rate: 0, time_to_restore_p50: null, observing_changes: 1 } }];
  report.releases[2].effectiveness = { window_days: 28, finished: 2, failed_changes: 1, observing: 1, change_failure_rate: 0.5 };
  state.environments = report;
  state.scope = "release-detail";
  const paths = [];
  globalThis.fetch = async path => { paths.push(path); return { ok: true, json: async () => ({ status: "ok", data: { detail: detailFixture() } }) }; };
  await loadReleaseDetail("factory", "b".repeat(40), "d1");
  assert.equal(paths[0], releaseDetailQuery("factory", "b".repeat(40), "d1"));
  const html = el.environments.innerHTML;
  assert.match(html, /Captured changes/);
  assert.match(html, /release &lt;unsafe&gt;/);
  assert.match(html, /github\.com\/owner\/repo\/pull\/90/);
  assert.match(html, /Issue reference #91/);
  assert.match(html, /Reference #92/);
  assert.match(html, /bounded list truncated at 200 commits/);
  assert.match(html, /\/api\/runs\/build%2Frun\/provenance/);
  assert.match(html, /1 recorded attestations/);
  assert.match(html, /product &lt;unsafe&gt;/);
  assert.match(html, /\/api\/dependencies\/documents\/sbom%2Fid\?scope=factory/);
  assert.match(html, /Release effectiveness over time/);
  assert.match(html, /1 still observing; provisional CFR/);
  assert.match(html, /50.0% · 1\/2 changes · 28d · 1 still observing/);
  state.scope = "elsewhere";
  renderEnvironments();
  assert.doesNotMatch(el.environments.innerHTML, /Captured changes/);
  state.scope = null;
});

test("late release evidence cannot replace a newer selection, and missing evidence is explicit", async () => {
  const el = stubPage(IDS);
  state.environments = REPORT;
  state.scope = "release-race";
  const finishes = [];
  globalThis.fetch = async () => { const detail = await new Promise(resolve => finishes.push(resolve));
    return { ok: true, json: async () => ({ status: "ok", data: { detail } }) }; };
  const first = loadReleaseDetail("factory", REPORT.releases[0].commit);
  const second = loadReleaseDetail("factory", REPORT.releases[2].commit);
  const missing = { release: REPORT.releases[2], sboms: [], build_reason: "no matching build <proof>", sbom_reason: "no matching SBOM" };
  finishes[1](missing);
  await second;
  finishes[0](detailFixture());
  await first;
  assert.match(el.environments.innerHTML, /no matching build &lt;proof&gt;/);
  assert.match(el.environments.innerHTML, /No Git comparison was captured/);
  assert.doesNotMatch(el.environments.innerHTML, /Captured changes/);
  wireEnvironments();
  el.environments.onclick({ target: { closest: selector => selector === "[data-release-close]" ? {} : null } });
  assert.doesNotMatch(el.environments.innerHTML, /no matching build/);
  state.scope = null;
});
