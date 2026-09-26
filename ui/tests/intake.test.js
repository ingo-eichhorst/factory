import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { state } from "../js/core.js";

// `modal.js` listens for Escape on the document as it loads, so there has
// to be one before the view is imported -- `operations.test.js`'s rule.
const bare = { addEventListener() {}, getElementById: () => null };
globalThis.document = bare;
const { loadIntake, showIntake, hideIntake, refreshIntake, intakeCard } = await import("../js/intake.js");

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

test("an item still in intake never lands on the Tasks board's closed column", () => {
  assert.match(tasks, /if \(t\.status !== "intake"\) byCol\.get\(columnFor\(t\)\)\.push\(t\);/);
  assert.match(css, /\.s-intake \{/);
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
