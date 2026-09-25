import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { state } from "../js/core.js";

// `modal.js`, which the view opens its confirmations in, listens for Escape
// on the document as it loads -- so there has to be one before the view is
// imported, the same requirement `scenarios.test.js` documents.
const bare = { addEventListener() {}, getElementById: () => null };
globalThis.document = bare;
const { loadOperations, showOperations, hideOperations } = await import("../js/operations.js");
const { loadInbox } = await import("../js/dashboard.js");

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const wiring = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");
const view = readFileSync(new URL("../js/operations.js", import.meta.url), "utf8");
const dashboard = readFileSync(new URL("../js/dashboard.js", import.meta.url), "utf8");
const tasks = readFileSync(new URL("../js/tasks.js", import.meta.url), "utf8");
const served = readFileSync(new URL("../../crates/factory-daemon/src/ui.rs", import.meta.url), "utf8");
const REPORT = JSON.parse(readFileSync(new URL("./fixtures/operations-report.json", import.meta.url), "utf8"));

// ------------------------------------------------------------- the frame

test("Operations is L4's third tab, after Tasks and Workflows, and is served", () => {
  assert.match(page, /id="tab-workflows"[^>]*>Workflows<\/button>\s*<button id="tab-operations"[^>]*>Operations<\/button>/);
  assert.match(page, /id="view-operations"/);
  assert.match(page, /<span class="lv-sub">Tasks, workflows, operations<\/span>/);
  assert.match(wiring, /proc: \["tasks", "workflows", "operations"\]/);
  assert.match(wiring, /operations: \{ onShow: showOperations, onHide: hideOperations \}/);
  assert.match(served, /"js\/operations\.js"/);
  assert.match(served, /"js\/operations-model\.js"/);
});

test("no browser dialogs: every action is confirmed in the app's own modal", () => {
  const code = view.split("\n").filter((l) => !l.trim().startsWith("//")).join("\n");
  assert.doesNotMatch(code, /(?<![.\w])(alert|confirm|prompt)\(/);
  assert.match(view, /scrim\(/);
});

test("live updates are events, coalesced, and only for a tab someone is looking at", () => {
  assert.match(wiring, /const OPS_REFRESH_MS = 1500;/);
  assert.match(wiring, /if \(state\.tab !== "operations" && state\.tab !== "inbox"\) return;/);
  assert.match(wiring, /if \(document\.hidden\) return;/);
  assert.match(wiring, /ev\.type !== "agent_activity"/);
  assert.doesNotMatch(view, /setInterval/, "no poll");
});

test("the Inbox no longer derives its own list from the task list", () => {
  assert.doesNotMatch(dashboard, /t\.status === "blocked"\) out\.push/);
  assert.doesNotMatch(dashboard, /entries\?limit=20/, "the blocked reason arrives with the exception");
  assert.match(dashboard, /api\("\/api\/operations"\)/);
  assert.match(wiring, /inbox: \{ onShow: loadInbox \}/);
});

test("a paused schedule is marked wherever a scheduled task is drawn", () => {
  // Card, table row and modal meta all go through the one helper.
  assert.equal((tasks.match(/\$\{pausedTag\(t\)\}/g) || []).length, 3);
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

function answering(report, requested) {
  return async (path) => {
    requested.push(path);
    return { status: 200, statusText: "OK", json: async () => ({ status: "ok", data: { kind: "operations", report } }) };
  };
}

test("the tab renders every section from one unscoped read", async () => {
  const el = stubPage(["ops", "ops-error", "ops-generated"]);
  const requested = [];
  globalThis.fetch = answering(REPORT, requested);
  state.scope = null;
  state.tasks = new Map();
  showOperations();
  await loadOperations();
  assert.deepEqual([...new Set(requested)], ["/api/operations?window=7d"], "no scope selected: health comes from the same read");
  const html = el.ops.innerHTML;
  for (const heading of ["Needs attention", "Flow now", "Aging WIP", "Process health", "Schedules"]) {
    assert.ok(html.includes(heading), heading);
  }
  assert.match(html, /First look from this browser/);
  assert.match(html, /which vault entry should I use\?/, "the reason in the agent's words");
  assert.match(html, /capacity unknown/);
  assert.match(html, /data-act="answer"/);
  assert.match(html, /Europe\/Berlin/);
  assert.match(html, /Aging WIP as a table/);
  assert.match(el["ops-generated"].textContent, /^as of 2026-09-25 12:39 UTC$/);
  hideOperations();
});

test("a selected scope adds a scoped read for health and says what it covers", async () => {
  const el = stubPage(["ops", "ops-error", "ops-generated"]);
  const requested = [];
  globalThis.fetch = answering(REPORT, requested);
  state.scope = "alpha";
  state.scopes = [];
  await loadOperations();
  assert.ok(requested.includes("/api/operations?window=7d"));
  assert.ok(requested.includes("/api/operations?window=7d&scope=alpha"));
  assert.match(el.ops.innerHTML, /Health covers alpha itself, not the scopes nested under it/);
  state.scope = null;
});

test("a suspicion is words beside the kind, never a status badge", async () => {
  const el = stubPage(["ops", "ops-error", "ops-generated"]);
  const suspected = {
    ...REPORT,
    attention: [{ kind: "suspected_stuck", severity: "medium", scope: "beta", task_id: "t", title: "quiet one", run_id: "r", since: REPORT.generated_at, age_s: 0, reason: "no journal progress for 40m", actions: ["cancel"], suspicion: true }],
  };
  globalThis.fetch = answering(suspected, []);
  await loadOperations();
  assert.match(el.ops.innerHTML, /suspicion, not a status/);
  assert.doesNotMatch(el.ops.innerHTML, /class="badge[^"]*"[^>]*>may be stuck/);
});

test("an empty queue says so plainly", async () => {
  const el = stubPage(["ops", "ops-error", "ops-generated"]);
  globalThis.fetch = answering({ ...REPORT, attention: [] }, []);
  await loadOperations();
  assert.match(el.ops.innerHTML, /Nothing needs you\./);
});

test("a failed read shows the daemon's message and draws nothing stale", async () => {
  const el = stubPage(["ops", "ops-error", "ops-generated"]);
  globalThis.fetch = async () => ({ status: 404, statusText: "Not Found", json: async () => ({ status: "error", code: "no_such_scope", message: "no such scope: nowhere" }) });
  await loadOperations();
  assert.equal(el["ops-error"].textContent, "no such scope: nowhere");
  assert.equal(el["ops-error"].hidden, false);
  assert.equal(el.ops.innerHTML, "");
});

test("the Inbox lists the attention queue, every scope, with the reason inline", async () => {
  const el = stubPage(["inbox"]);
  const requested = [];
  globalThis.fetch = answering(REPORT, requested);
  state.scope = "gamma"; // the Inbox ignores the rail
  await loadInbox();
  assert.deepEqual(requested, ["/api/operations"]);
  const html = el.inbox.innerHTML;
  assert.equal((html.match(/class="inbox-item"/g) || []).length, REPORT.attention.length);
  assert.match(html, /summarise support inbox<\/b> — blocked/);
  assert.match(html, /which vault entry should I use\? · alpha/);
  state.scope = null;
});

test("the Inbox says when there is nothing, and when it could not ask", async () => {
  const el = stubPage(["inbox"]);
  globalThis.fetch = answering({ ...REPORT, attention: [] }, []);
  await loadInbox();
  assert.match(el.inbox.innerHTML, /Nothing waiting on a person right now\./);
  globalThis.fetch = async () => { throw new Error("offline"); };
  await loadInbox();
  assert.match(el.inbox.innerHTML, /not available right now/);
});
