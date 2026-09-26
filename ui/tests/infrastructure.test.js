import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  MISSING,
  PROVIDERS_SNIPPET,
  agentHref,
  daemonUptime,
  diskLevel,
  diskPercent,
  fact,
  fmtBytes,
  fmtInterfaces,
  fmtLoad,
  fmtRuntime,
  fmtUptime,
  groupAgentsByScope,
  harnessDetail,
  harnessStateLabel,
  infraFailure,
  isEmptyProviders,
  kindBadge,
  providerWindowView,
  resetsIn,
  sortHarnesses,
  visibleAgents,
} from "../js/infra-model.js";

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const app = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");

// The issue's example answer, verbatim: the contract the daemon and the UI
// are both built against.
const EXAMPLE = {
  host: {
    hostname: "factory-mac",
    model: "Mac17,7",
    chip: "Apple M5 Max",
    cores: 18,
    memory_bytes: 68719476736,
    os: "macOS 26.6.1",
    arch: "aarch64",
    uptime_seconds: 864000,
    load: [2.1, 1.8, 1.6],
    disk: { mount: "/", total_bytes: 1995218165760, free_bytes: 1539316654080 },
  },
  daemon: {
    version: "0.1.0",
    pid: 4242,
    started_at: "2026-09-24T08:00:00Z",
    root: "/Users/factory/business-factory",
    store: { kind: "sqlite", path: ".factory/factory.sqlite", size_bytes: 12582912 },
    socket: ".factory/factory.sock",
    interfaces: [{ kind: "cli" }, { kind: "http", bind: "192.168.188.92:8791" }],
    runtime: "herdr",
    herdr_session: "factory",
  },
  providers: [
    {
      name: "claude-max",
      vendor: "anthropic",
      kind: "subscription",
      plan: "Max 20x",
      env: null,
      agents: [{ scope: "factory", agent: "claude-code", harness: "claude-code", via: "harness" }],
    },
  ],
  unassigned: [{ scope: "model-lab", agent: "model-lab", harness: "opencode" }],
};

// Every host fact the daemon could not read, as the wire spells that: null.
const NULL_HOST = {
  hostname: null, model: null, chip: null, cores: null, memory_bytes: null, os: null,
  arch: null, uptime_seconds: null, load: null, disk: null,
};

// ------------------------------------------------------------------ the level

test("L1 Infrastructure is live, with its sub-label, tab and view", () => {
  assert.doesNotMatch(page, /id="lv-infra"[^>]*disabled/);
  assert.doesNotMatch(page, /id="lv-infra"[^>]*Not built yet/);
  assert.match(page, /id="lv-infra"[\s\S]*?<span class="lv-sub">Host, daemon, Doctor and backup<\/span>/);
  assert.match(page, /id="tab-infrastructure"[^>]*>Infrastructure<\/button>/);
  assert.match(page, /id="view-infrastructure"/);
});

test("LEVEL_VIEWS.infra names Infrastructure first, and the view is registered", () => {
  assert.match(app, /infra: \["infrastructure", "doctor", "backup"\]/);
  assert.match(app, /infrastructure: \{ onShow: startInfrastructure, onHide: stopAgentPoll \}/);
  assert.match(app, /state\.tab === "infrastructure"/, "the rail's re-render names every tab");
});

test("a daemon without the endpoint gets a calm note of its own, apart from the error", () => {
  assert.match(page, /id="infrastructure-unavailable"[^>]*hidden/);
  assert.match(page, /id="infrastructure-error"[^>]*hidden/);
  assert.doesNotMatch(page, /class="env-note bad" id="infrastructure-unavailable"/);
});

// ------------------------------------------------------------------ the host

test("the example host formats the way the machine describes itself", () => {
  const h = EXAMPLE.host;
  assert.equal(fmtBytes(h.memory_bytes), "64 GB");
  assert.equal(fmtBytes(h.disk.total_bytes), "1.8 TB");
  assert.equal(fmtUptime(h.uptime_seconds), "10d 0h");
  assert.equal(fmtLoad(h.load), "2.10 · 1.80 · 1.60");
  assert.equal(fact(h.cores), "18");
  assert.equal(fact(h.chip), "Apple M5 Max");
});

test("the disk percentage is the used share, rounded", () => {
  // (1995218165760 - 1539316654080) / 1995218165760 = 22.85%
  assert.equal(diskPercent(EXAMPLE.host.disk), 23);
  assert.equal(diskLevel(23), "ok");
  assert.equal(diskLevel(80), "warn");
  assert.equal(diskLevel(95), "full");
  assert.equal(diskPercent({ mount: "/", total_bytes: 100, free_bytes: 0 }), 100);
});

test("every null host fact is drawn as a dash, never NaN or zero", () => {
  const h = NULL_HOST;
  for (const k of ["hostname", "model", "chip", "cores", "os", "arch"]) {
    assert.equal(fact(h[k]), MISSING, k);
  }
  assert.equal(fmtBytes(h.memory_bytes), MISSING);
  assert.equal(fmtUptime(h.uptime_seconds), MISSING);
  assert.equal(fmtLoad(h.load), MISSING);
  assert.equal(diskPercent(h.disk), null);
  assert.equal(diskLevel(null), null);
});

test("a disk whose size or free space is unknown has no percentage", () => {
  assert.equal(diskPercent({ mount: "/", total_bytes: null, free_bytes: 5 }), null);
  assert.equal(diskPercent({ mount: "/", total_bytes: 100, free_bytes: null }), null);
  assert.equal(diskPercent({ mount: "/", total_bytes: 0, free_bytes: 0 }), null);
});

test("a load triple with a hole keeps the members it has", () => {
  assert.equal(fmtLoad([1.234, null, 0.5]), `1.23 · ${MISSING} · 0.50`);
  assert.equal(fmtLoad([]), MISSING);
});

test("bytes and spans at the edges", () => {
  assert.equal(fmtBytes(0), "0 B");
  assert.equal(fmtBytes(1023), "1023 B");
  assert.equal(fmtBytes(1024), "1 KB");
  assert.equal(fmtBytes(1536), "1.5 KB");
  assert.equal(fmtBytes(12582912), "12 MB");
  assert.equal(fmtBytes(-1), MISSING);
  assert.equal(fmtBytes(undefined), MISSING);
  assert.equal(fmtUptime(0), "0s");
  assert.equal(fmtUptime(59), "59s");
  assert.equal(fmtUptime(61), "1m");
  assert.equal(fmtUptime(3600 + 12 * 60), "1h 12m");
  assert.equal(fmtUptime(86400 + 3 * 3600), "1d 3h");
});

// ------------------------------------------------------------------ the daemon

test("the daemon's uptime is counted from its start, against the caller's clock", () => {
  const now = Date.parse("2026-09-24T10:30:00Z");
  assert.equal(daemonUptime(EXAMPLE.daemon.started_at, now), 9000);
  assert.equal(fmtUptime(daemonUptime(EXAMPLE.daemon.started_at, now)), "2h 30m");
  assert.equal(daemonUptime(null, now), null);
  assert.equal(daemonUptime("not a date", now), null);
  assert.equal(daemonUptime("2026-09-25T00:00:00Z", now), null, "a start in the future is no uptime");
});

test("the daemon's interfaces, store size and runtime read as one line each", () => {
  assert.equal(fmtInterfaces(EXAMPLE.daemon.interfaces), "cli · http 192.168.188.92:8791");
  assert.equal(fmtInterfaces([]), MISSING);
  assert.equal(fmtInterfaces(null), MISSING);
  assert.equal(fmtBytes(EXAMPLE.daemon.store.size_bytes), "12 MB");
  assert.equal(fmtRuntime(EXAMPLE.daemon), "herdr · session factory");
  assert.equal(fmtRuntime({ runtime: "herdr", herdr_session: null }), "herdr");
  assert.equal(fmtRuntime(null), MISSING);
});

// ------------------------------------------------------------------ providers

test("the example has a provider, so there is no empty state", () => {
  assert.equal(isEmptyProviders(EXAMPLE), false);
  assert.equal(kindBadge(EXAMPLE.providers[0].kind), "subscription");
  assert.equal(kindBadge("api-key"), "api-key");
  assert.equal(kindBadge(null), MISSING);
});

test("no providers is the empty state, and it shows the config block to add", () => {
  assert.equal(isEmptyProviders({ ...EXAMPLE, providers: [] }), true);
  assert.equal(isEmptyProviders({ host: EXAMPLE.host }), true);
  assert.equal(isEmptyProviders(null), true);
  assert.match(PROVIDERS_SNIPPET, /^infrastructure:\n {2}providers:/m);
  assert.match(PROVIDERS_SNIPPET, /kind: subscription/);
  assert.match(PROVIDERS_SNIPPET, /env: OPENROUTER_API_KEY/);
});

test("agents group by scope in the order the daemon listed them", () => {
  const agents = [
    { scope: "factory", agent: "claude-code", harness: "claude-code", via: "harness" },
    { scope: "assistant", agent: "pi", harness: "pi", via: "harness" },
    { scope: "factory", agent: "reviewer", harness: "claude-code", via: "agent" },
  ];
  const groups = groupAgentsByScope(agents);
  assert.deepEqual(groups.map(g => g.scope), ["factory", "assistant"]);
  assert.deepEqual(groups[0].agents.map(a => a.agent), ["claude-code", "reviewer"]);
  assert.deepEqual(groupAgentsByScope(EXAMPLE.providers[0].agents).map(g => g.scope), ["factory"]);
  assert.deepEqual(groupAgentsByScope(undefined), []);
});

test("unassigned agents group the same way and narrow to the rail", () => {
  const rows = [
    ...EXAMPLE.unassigned,
    { scope: "factory", agent: "opencode", harness: "opencode" },
  ];
  assert.deepEqual(groupAgentsByScope(rows).map(g => g.scope), ["model-lab", "factory"]);
  assert.deepEqual(visibleAgents(rows, s => s === "model-lab").map(a => a.agent), ["model-lab"]);
  assert.equal(visibleAgents(rows).length, 2, "no selection keeps every row");
  assert.deepEqual(visibleAgents(null, () => true), []);
});

test("an agent links to the roster with its scope selected", () => {
  assert.equal(agentHref("factory"), "#factory/roster");
});

test("provider windows distinguish measured, stale, apportioned, and unknown", () => {
  const measured = providerWindowView({
    window_minutes: 300, used_percent: 42.25, stale: false,
    attribution: "apportioned", attribution_quality: "confirmed", trend_percent: 3.5,
  });
  assert.equal(measured.name, "5-hour");
  assert.equal(measured.percent, 42.25);
  assert.match(measured.detail, /apportioned/);
  assert.match(measured.detail, /\+3\.5 points/);
  const stale = providerWindowView({ window_minutes: 10080, used_percent: 80, stale: true });
  assert.equal(stale.name, "weekly");
  assert.equal(stale.tone, "stale");
  const unknown = providerWindowView({ window_minutes: 300, used_percent: null, unknown: "no baseline" });
  assert.equal(unknown.percent, null);
  assert.equal(unknown.tone, "unknown");
  assert.match(unknown.detail, /no baseline/);
});

test("a provider window says when it resets, relative to now, and never guesses", () => {
  const now = Date.parse("2026-09-26T12:00:00Z");
  assert.equal(resetsIn("2026-09-26T14:10:00Z", now), "resets in 2h 10m");
  assert.equal(resetsIn("2026-09-26T12:00:30Z", now), "resets in 1m");
  assert.equal(resetsIn("2026-09-29T15:00:00Z", now), "resets in 3d 3h");
  assert.equal(resetsIn("2026-09-26T11:00:00Z", now), "reset passed");
  assert.equal(resetsIn("not a time", now), null);
  assert.equal(resetsIn(undefined, now), null);
  const view = providerWindowView({ window_minutes: 300, used_percent: 10, resets_at: "2026-09-26T14:10:00Z" }, now);
  assert.match(view.detail, /resets in 2h 10m/);
});

test("a provider window's badge is the sampled interval's, and an unattributable change says why", () => {
  const direct = providerWindowView({ window_minutes: 300, used_percent: 10, attribution: "direct" });
  assert.match(direct.detail, /direct/);
  const unknown = providerWindowView({
    window_minutes: 300, used_percent: 10, attribution_unknown: "no earlier sample of this window",
  });
  assert.match(unknown.detail, /attribution unknown — no earlier sample of this window/);
  const undated = providerWindowView({ window_minutes: 300, used_percent: 10, stale: true, sample_time_estimated: true });
  assert.equal(undated.tone, "stale");
  assert.match(undated.detail, /sample time not reported/);
});

// ------------------------------------------------------------------ fetch failures

test("a bare 404 is a daemon that predates the endpoint, not an error", () => {
  // `api()` turns a body-less response into "<status> <statusText>".
  assert.equal(infraFailure(new Error("404 Not Found")), "unavailable");
  assert.equal(infraFailure(new Error("500 Internal Server Error")), "error");
  assert.equal(infraFailure(new Error("Failed to fetch")), "error");
  assert.equal(infraFailure(new Error("permission denied: 404 of them")), "error");
  assert.equal(infraFailure(undefined), "error");
});

// -- harnesses (#131) --------------------------------------------------------

const STUCK = {
  harness: "codex",
  binary: "/opt/homebrew/bin/codex",
  state: "unhealthy",
  reason: "`/opt/homebrew/bin/codex --version` did not answer in 10s",
  repair: "scripts/repair-harness codex",
  held: [{ task_id: "t1", scope: "demo", title: "fix it" }],
};

test("a harness says whether it starts, and never calls an unprobed one broken", () => {
  assert.equal(harnessStateLabel("healthy"), "starts");
  assert.equal(harnessStateLabel("unhealthy"), "does not start");
  assert.equal(harnessStateLabel("unprobed"), "not checked yet");
  assert.equal(harnessStateLabel("something-new"), "something-new");
  assert.equal(harnessStateLabel(null), MISSING);
});

test("a harness's detail is its version, its reason, or a promise to check", () => {
  assert.equal(harnessDetail({ state: "healthy", version: "codex-cli 0.157.0" }), "codex-cli 0.157.0");
  assert.match(harnessDetail(STUCK), /did not answer in 10s/);
  assert.match(harnessDetail({ state: "unprobed" }), /before the next dispatch/);
  assert.equal(harnessDetail(null), MISSING);
});

test("an unhealthy harness sorts first, the rest keep the daemon's order", () => {
  const rows = [
    { harness: "claude", state: "healthy" },
    { harness: "pi", state: "unprobed" },
    STUCK,
  ];
  assert.deepEqual(sortHarnesses(rows).map(r => r.harness), ["codex", "claude", "pi"]);
  assert.deepEqual(sortHarnesses(undefined), []);
});

test("the page draws the harness layer from the payload's harnesses", () => {
  const view = readFileSync(new URL("../js/infrastructure.js", import.meta.url), "utf8");
  assert.match(view, /data\.harnesses/);
  assert.match(view, /h\.repair/);
});
