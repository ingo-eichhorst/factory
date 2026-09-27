import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { state } from "../js/core.js";

// `modal.js` listens for Escape on the document as it loads, so there has
// to be one before the view is imported -- `operations.test.js`'s rule.
const bare = { addEventListener() {}, getElementById: () => null };
globalThis.document = bare;
const { loadIntake, showIntake, hideIntake, refreshIntake, intakeCard, candidatesBlock } = await import("../js/intake.js");

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const wiring = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");
const view = readFileSync(new URL("../js/intake.js", import.meta.url), "utf8");
const tasks = readFileSync(new URL("../js/tasks.js", import.meta.url), "utf8");
const css = readFileSync(new URL("../app.css", import.meta.url), "utf8");
const served = readFileSync(new URL("../../crates/factory-daemon/src/ui.rs", import.meta.url), "utf8");
const DATA = JSON.parse(readFileSync(new URL("./fixtures/intake-board.json", import.meta.url), "utf8"));

// ------------------------------------------------------------- the frame

test("Intake is L4's second tab, beside Tasks, and is served", () => {
  assert.match(page, /id="tab-tasks"[^>]*>Tasks<\/button>\s*<button id="tab-intake"[^>]*>Intake<\/button>/);
  assert.match(page, /id="view-intake"/);
  assert.match(page, /id="intake-add"/);
  assert.match(wiring, /proc: \["tasks", "intake", "workflows", "operations"\]/);
  assert.match(wiring, /intake: \{ onShow: showIntake, onHide: hideIntake \}/);
  assert.match(wiring, /if \(touchesIntake\(ev\)\) refreshIntake\(\);/);
  assert.match(served, /"js\/intake\.js"/);
  assert.match(served, /"js\/intake-model\.js"/);
});

test("no browser dialogs, no poll, and nothing posted outside Factory", () => {
  const code = view.split("\n").filter((l) => !l.trim().startsWith("//")).join("\n");
  assert.doesNotMatch(code, /(?<![.\w])(alert|confirm|prompt)\(/);
  assert.doesNotMatch(view, /setInterval/);
  assert.match(view, /scrim\(/);
  for (const path of code.match(/\/api\/[a-z/]+/g) || []) assert.match(path, /^\/api\/intake/, path);
});

test("the assess dialog renders the routed scope's own extra checks and holds the assessment to them (#169)", () => {
  assert.match(view, /function checksHtml\(route, prior\)/);
  assert.match(view, /data-check="/);
  assert.match(view, /data-check-evidence="/);
  assert.match(view, /\$\("ik-checks"\)\.innerHTML = checksHtml\(r, prior\);/);
  assert.match(view, /assessmentProblem\(a, candidates, definition\)/);
  assert.match(view, /previewVerdict\(a, definition\)/);
});

test("an item still in intake never lands on the Tasks board's closed column", () => {
  assert.match(tasks, /if \(t\.status !== "intake"\) byCol\.get\(columnFor\(t\)\)\.push\(t\);/);
  assert.match(css, /\.s-intake \{/);
});

test("the security fast lane has its own badge, band styling and owner dialogs (#170)", () => {
  assert.match(css, /\.ik-fast-lane \{/);
  assert.match(css, /\.ik-sec-possible \{/);
  assert.match(css, /\.ik-sec-confirmed \{/);
  assert.match(css, /\.ik-sec-dismissed \{/);
  assert.match(view, /function openFlagSecurityDialog\(card\)/);
  assert.match(view, /function openSecurityDecisionDialog\(verdict, card\)/);
  assert.match(view, /flagSecurityRequest\(card\.id/);
  assert.match(view, /securityDecisionRequest\(card\.id, verdict/);
});

test("approving a GitHub publish is its own dialog, journaled through the daemon like every other action (#171)", () => {
  assert.match(view, /function openPublishDialog\(card\)/);
  assert.match(view, /publishRequest\(card\.id\)/);
  assert.match(view, /if \(action === "publish"\) return openPublishDialog\(card\);/);
});

// ------------------------------------------------------------- rendering

function stubElement() {
  return { innerHTML: "", textContent: "", hidden: false, querySelectorAll: () => [] };
}

function stubPage(ids) {
  const elements = {};
  for (const id of ids) elements[id] = stubElement();
  globalThis.document = { ...bare, getElementById: (id) => elements[id] || null };
  return elements;
}

function answering(data, requested) {
  return async (path) => {
    requested.push(path);
    return { status: 200, statusText: "OK", json: async () => ({ status: "ok", data }) };
  };
}

const IDS = ["intake", "intake-error", "intake-summary"];

test("the board draws four columns from one read, each card with its axes and verdict", async () => {
  const el = stubPage(IDS);
  const requested = [];
  globalThis.fetch = answering(DATA, requested);
  state.scope = null;
  showIntake();
  await Promise.resolve();
  await loadIntake();
  assert.deepEqual([...new Set(requested)], ["/api/intake"]);
  const html = el.intake.innerHTML;
  for (const label of ["Received", "Triaging", "Needs info", "Ready"]) assert.ok(html.includes(`<span>${label}</span>`), label);
  for (const key of ["ik-received", "ik-triaging", "ik-needs_info", "ik-ready"]) assert.ok(html.includes(`data-col="${key}"`), key);
  assert.match(html, /Order page shows stale totals/);
  assert.match(html, /from Kim \(customer\)/);
  assert.match(html, /class="badge ik-p ik-P2">P2</);
  assert.match(html, /45m-2h/);
  assert.match(html, /\? Who at the partner issues the new key\?/);
  assert.equal((html.match(/class="ik-axis /g) || []).length, 4 * 7, "seven marks on every card");
  assert.match(html, /ik-tolerated/);
  assert.match(el["intake-summary"].textContent, /^3 open · 1 released, 1 split and 1 closed in the last 14 days$/);
  assert.equal(el["intake-error"].hidden, true);
  hideIntake();
});

test("a selected scope is one scoped read", async () => {
  stubPage(IDS);
  const requested = [];
  globalThis.fetch = answering(DATA, requested);
  state.scope = "web";
  await loadIntake();
  assert.deepEqual(requested, ["/api/intake?scope=web"]);
  state.scope = null;
});

test("a refresh only reads while the view is on screen", async () => {
  stubPage(IDS);
  const requested = [];
  globalThis.fetch = answering(DATA, requested);
  hideIntake();
  refreshIntake();
  await Promise.resolve();
  assert.deepEqual(requested, []);
});

test("a failed read shows the daemon's message and draws nothing stale", async () => {
  const el = stubPage(IDS);
  globalThis.fetch = async () => ({ status: 400, statusText: "Bad Request", json: async () => ({ status: "error", code: "bad_request", message: "no such scope: nowhere" }) });
  await loadIntake();
  assert.equal(el["intake-error"].hidden, false);
  assert.equal(el["intake-error"].textContent, "no such scope: nowhere");
  assert.equal(el.intake.innerHTML, "");
});

test("a card escapes what a requester typed", () => {
  const evil = { ...DATA.board.columns.received[0], title: "<img src=x onerror=alert(1)>", requester: "<b>x</b>" };
  const html = intakeCard(evil, DATA.board.axes);
  assert.doesNotMatch(html, /<img/);
  assert.doesNotMatch(html, /<b>x/);
});

test("a card's estimate chip carries its reference-class basis as a tooltip (#168)", () => {
  const withBasis = {
    ...DATA.board.columns.ready[0],
    triage: {
      ...DATA.board.columns.ready[0].triage,
      estimate_basis: { source: "reference_class", scope: "web", category: "bugfix", time_samples: 12 },
    },
  };
  const html = intakeCard(withBasis, DATA.board.axes);
  assert.match(html, /title="p10–p90 of 12 completed bugfix tasks in web, last 90 days"/);
});

test("a card's estimate chip carries the complexity-table fallback as a tooltip (#168)", () => {
  const fallback = {
    ...DATA.board.columns.ready[0],
    triage: {
      ...DATA.board.columns.ready[0].triage,
      estimate_basis: { source: "complexity_table", scope: "web", category: "bugfix", time_samples: 2 },
    },
  };
  const html = intakeCard(fallback, DATA.board.axes);
  assert.match(html, /title="complexity table: 2 of 5 samples"/);
});

test("a card from before #168 has no estimate_basis and shows no tooltip", () => {
  const html = intakeCard(DATA.board.columns.ready[0], DATA.board.axes);
  assert.match(html, /class="sub" title="">45m-2h/);
});

test("a card flagged as a possible security report carries the badge and the fast-lane band (#170)", () => {
  const flagged = {
    ...DATA.board.columns.received[0],
    security: { state: "possible", flagged_by: "agent triager", flagged_at: "t", reason: "looks exploitable" },
  };
  const html = intakeCard(flagged, DATA.board.axes);
  assert.match(html, /ik-fast-lane/);
  assert.match(html, /ik-sec-possible/);
  assert.match(html, /possible security report/);
  assert.match(html, /title="looks exploitable"/);
});

test("a card with no security flag draws no badge and no fast-lane band", () => {
  const html = intakeCard(DATA.board.columns.received[0], DATA.board.axes);
  assert.doesNotMatch(html, /ik-fast-lane/);
  assert.doesNotMatch(html, /ik-sec-/);
});

test("a card's security reason is escaped", () => {
  const evil = {
    ...DATA.board.columns.received[0],
    security: { state: "confirmed", flagged_by: "<b>x</b>", flagged_at: "t", reason: "<img src=x onerror=alert(1)>" },
  };
  const html = intakeCard(evil, DATA.board.axes);
  assert.doesNotMatch(html, /<img/);
});

test("a decided GitHub card shows its outbound state, awaiting, published or failed (#171)", () => {
  const github = (outbound) => ({
    ...DATA.board.columns.needs_info[0],
    source: { kind: "github", reference: "https://github.com/acme/widgets/issues/9" },
    outbound,
  });
  assert.match(intakeCard(github({ state: "awaiting_approval", by: "the owner", at: "t" }), DATA.board.axes), /awaiting approval/);
  assert.match(intakeCard(github({ state: "published", by: "the owner", at: "t" }), DATA.board.axes), /published to GitHub/);
  assert.match(intakeCard(github({ state: "failed", by: "the owner", at: "t" }), DATA.board.axes), /GitHub publish failed/);
  assert.doesNotMatch(intakeCard(DATA.board.columns.needs_info[0], DATA.board.axes), /GitHub/, "nothing outbound, nothing shown");
});

// ------------------------------------------------------------ duplicates (#166)

test("the item modal lists each possible duplicate with its match and verdict", () => {
  const card = { candidates: [
    { kind: "task", reference: "t-1", title: "Order page shows stale totals", evidence: "same reference", match: "source", verdict: "unverified" },
    { kind: "knowledge", reference: "specs/x.md", title: "X design", evidence: "documents the same flow", match: "text", score: 82, verdict: "confirmed" },
  ] };
  const html = candidatesBlock(card);
  assert.match(html, /Possible duplicates/);
  assert.match(html, /t-1/);
  assert.match(html, /ik-dup-unverified/);
  assert.match(html, /ik-dup-confirmed/);
  assert.match(html, /text 82%/);
  assert.equal(candidatesBlock({}), "", "nothing to show, nothing drawn");
});

test("the item modal escapes a candidate's own title and evidence", () => {
  const evil = { candidates: [
    { kind: "task", reference: "t-1", title: "<img src=x onerror=alert(1)>", evidence: "<b>evil</b>", match: "source", verdict: "unverified" },
  ] };
  const html = candidatesBlock(evil);
  assert.doesNotMatch(html, /<img/);
  assert.doesNotMatch(html, /<b>evil/);
});
