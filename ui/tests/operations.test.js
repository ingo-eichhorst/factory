import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { state } from "../js/core.js";

// `modal.js`, which the view opens its confirmations in, listens for Escape
// on the document as it loads -- so there has to be one before the view is
// imported, the same requirement `scenarios.test.js` documents.
const bare = { addEventListener() {}, getElementById: () => null };
globalThis.document = bare;
const { loadOperations, showOperations, hideOperations, openActionDialog } = await import("../js/operations.js");
const { loadInbox } = await import("../js/dashboard.js");

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const wiring = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");
const view = readFileSync(new URL("../js/operations.js", import.meta.url), "utf8");
const dashboard = readFileSync(new URL("../js/dashboard.js", import.meta.url), "utf8");
const tasks = readFileSync(new URL("../js/tasks.js", import.meta.url), "utf8");
const served = readFileSync(new URL("../../crates/factory-daemon/src/ui.rs", import.meta.url), "utf8");
const REPORT = JSON.parse(readFileSync(new URL("./fixtures/operations-report.json", import.meta.url), "utf8"));

// ------------------------------------------------------------- the frame

test("Line is L4's last tab, after Tasks, Intake and Workflows, and is served", () => {
  // `#185` renamed it from Operations, which now means the running systems.
  assert.match(page, /id="tab-workflows"[^>]*>Workflows<\/button>\s*<button id="tab-line"[^>]*>Line<\/button>/);
  assert.match(page, /id="view-line"/);
  assert.match(page, /<div id="view-line" hidden>\s*<div class="bar">\s*<h2>Line<\/h2>/);
  assert.doesNotMatch(page, /id="tab-operations"/);
  assert.match(page, /<span class="lv-sub">Tasks, intake, workflows, line<\/span>/);
  assert.match(wiring, /proc: \["tasks", "intake", "workflows", "line"\]/);
  assert.match(wiring, /line: \{ onShow: showOperations, onHide: hideOperations \}/);
  // An old link to the tab lands on it under its new name.
  assert.match(wiring, /operations: \(tail\) => \(\{ page: "line", tail \}\)/);
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
  assert.match(wiring, /if \(state\.tab !== "line" && state\.tab !== "inbox"\) return;/);
  assert.match(wiring, /if \(document\.hidden\) return;/);
  assert.match(wiring, /ev\.type !== "agent_activity"/);
  assert.doesNotMatch(view, /setInterval/, "no poll");
});

test("the Inbox no longer derives its own list from the task list", () => {
  assert.doesNotMatch(dashboard, /t\.status === "blocked"\) out\.push/);
  assert.doesNotMatch(dashboard, /entries\?limit=20/, "the blocked reason arrives with the exception");
  assert.match(dashboard, /api\("\/api\/operations"\)/);
  assert.match(wiring, /inbox: \{ onShow: startInbox, onHide: stopAgentPoll \}/);
});

test("a policy_changed event reloads the Inbox too, since a clock submission can flip one of its rows to met (#157/#170 phase 2)", () => {
  assert.match(dashboard, /api\("\/api\/policy\/clock"\)/);
  assert.match(wiring, /ev\.type === "policy_changed" && state\.tab === "inbox"\) \{\s*loadInbox\(\)\.catch\(/,
    "event-driven refresh handles its promise rejection");
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
    if (path === "/api/important-dates") return { status: 200, statusText: "OK", json: async () => ({ status: "ok", data: { kind: "important_dates", report: { entries: [], overdue: 0, due_soon: 0, unknown: 0 } } }) };
    return { status: 200, statusText: "OK", json: async () => ({ status: "ok", data: { kind: "operations", report } }) };
  };
}

test("the tab renders every section from one read that asks for the charts", async () => {
  const el = stubPage(["ops", "ops-error", "ops-generated"]);
  const requested = [];
  globalThis.fetch = answering(REPORT, requested);
  state.scope = null;
  state.tasks = new Map();
  showOperations();
  await loadOperations();
  assert.deepEqual([...new Set(requested)], ["/api/operations?window=7d&detail=charts"]);
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

test("a selected scope is one scoped read: the daemon narrows to its subtree, the page does not", async () => {
  const el = stubPage(["ops", "ops-error", "ops-generated"]);
  const requested = [];
  globalThis.fetch = answering(REPORT, requested);
  state.scope = "alpha";
  state.scopes = [];
  await loadOperations();
  assert.deepEqual(requested, ["/api/operations?window=7d&detail=charts&scope=alpha"]);
  // Whatever the daemon answered is drawn -- a gamma row included -- since
  // the page no longer second-guesses which scopes are under the selection.
  assert.match(el.ops.innerHTML, /tidy stale branches/);
  assert.doesNotMatch(el.ops.innerHTML, /not the scopes nested under it/);
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
  // The reporting clock (`#157`/`#170` phase 2) is read alongside the
  // attention queue now -- `answering` hands both the same operations-
  // shaped body, so the clock side contributes nothing (`.clock` is
  // `undefined` on it), same as a daemon that predates the endpoint.
  assert.deepEqual([...requested].sort(), ["/api/important-dates", "/api/operations", "/api/policy/clock", "/api/signposts"]);
  const html = el.inbox.innerHTML;
  assert.equal((html.match(/class="inbox-item"/g) || []).length, REPORT.attention.length);
  assert.match(html, /summarise support inbox<\/b> — blocked/);
  assert.match(html, /which vault entry should I use\? · alpha/);
  state.scope = null;
});

test("the Inbox exposes approval decisions and evidence-backed rework", async () => {
  const el = stubPage(["inbox"]);
  const decisions = {
    ...REPORT,
    attention: [
      { ...REPORT.attention[0], actions: ["approve", "reject"] },
      { ...REPORT.attention[1], actions: ["accept_rework"] },
    ],
  };
  globalThis.fetch = answering(decisions, []);
  await loadInbox();
  assert.match(el.inbox.innerHTML, /class="btn inbox-action"/, "decisions use the shared button styling");
  assert.match(el.inbox.innerHTML, /data-action="approve"[^>]*>Approve/);
  assert.match(el.inbox.innerHTML, /data-action="reject"[^>]*>Reject/);
  assert.match(el.inbox.innerHTML, /data-action="accept_rework"[^>]*>Accept rework/);
  assert.match(dashboard, /window\.prompt\(`\$\{ACTION_LABELS\[action\]\} reason:`\)/, "approval and rejection capture evidence");
});

test("the Inbox says when there is nothing, and when it could not ask", async () => {
  const el = stubPage(["inbox"]);
  globalThis.fetch = answering({ ...REPORT, attention: [] }, []);
  await loadInbox();
  assert.match(el.inbox.innerHTML, /Nothing waiting on a person right now\./);
  globalThis.fetch = async () => { throw new Error("offline"); };
  await loadInbox();
  assert.match(el.inbox.innerHTML, /unavailable.*incomplete/);
});

test("the Inbox also lists the reporting clock's overdue and due-soon deadlines (#157/#170 phase 2)", async () => {
  const el = stubPage(["inbox"]);
  state.tasks = new Map([["task-1", { title: "Checkout crashes on coupon" }]]);
  const clock = {
    now: "2026-09-24T12:00:00Z",
    items: [
      {
        item: { kind: "report", item: "task-1" },
        scope: "web",
        awareness_at: "2026-09-20T12:00:00Z",
        reported_now: true,
        deadlines: [{ deadline: "notification", due_at: "2026-09-24T09:00:00Z", state: "overdue", submission: null }],
      },
      {
        item: { kind: "finding", scope: "demo", vulnerability: "CVE-2026-1234" },
        scope: "demo",
        awareness_at: "2026-09-23T12:00:00Z",
        reported_now: true,
        deadlines: [{ deadline: "early_warning", due_at: "2026-09-24T13:00:00Z", state: "due", submission: null }],
      },
      {
        // met -- never a to-do
        item: { kind: "finding", scope: "demo", vulnerability: "CVE-old" },
        scope: "demo",
        awareness_at: "2026-09-01T12:00:00Z",
        reported_now: true,
        deadlines: [{ deadline: "early_warning", due_at: "2026-09-02T12:00:00Z", state: "met", submission: { attestation: "a1", at: "2026-09-02T00:00:00Z", by: "owner" } }],
      },
    ],
  };
  globalThis.fetch = async (path) => {
    if (path === "/api/policy/clock") return { status: 200, statusText: "OK", json: async () => ({ status: "ok", data: { kind: "policy_clock", clock } }) };
    return { status: 200, statusText: "OK", json: async () => ({ status: "ok", data: { kind: "operations", report: { ...REPORT, attention: [
      { ...REPORT.attention[0], actions: ["approve", "reject"] },
      { ...REPORT.attention[1], actions: ["accept_rework"] },
    ] } } }) };
  };
  await loadInbox();
  const html = el.inbox.innerHTML;
  assert.equal((html.match(/class="inbox-item"/g) || []).length, 4, "two clock rows coexist with two decisions; the met deadline is not a to-do");
  // The overdue report leads (a fault outranks a warning), named by its
  // task's own title, not its bare id.
  assert.match(html, /Checkout crashes on coupon<\/b> — 72h notification, overdue/);
  assert.match(html, /overdue by/);
  // The due-soon finding names the vulnerability, since it has no task.
  assert.match(html, /CVE-2026-1234<\/b> — 24h early warning, due soon/);
  assert.match(html, /due in/);
  assert.match(html, /data-action="approve"/);
  assert.match(html, /data-action="reject"/);
  assert.match(html, /data-action="accept_rework"/);
  state.tasks = new Map();
});


test("the Dashboard and Inbox show escaped read-only signposts separately from waiting work", async () => {
  const el = stubPage(["inbox", "dash-signposts", "inbox-signposts"]);
  const requested = [];
  const ops = answering({ ...REPORT, attention: [] }, requested);
  globalThis.fetch = async (path) => {
    if (path === "/api/signposts") {
      requested.push(path);
      return { status: 200, statusText: "OK", json: async () => ({ status: "ok", data: { kind: "signposts", fact: {
        at: "2026-10-05T12:00:00Z", triggered: [{ scenario: "<slow>", metric: "throughput_week", reason: "<script>0 is below 999</script>", actions: ["approve"], task_id: "must-not-open" }],
      } } }) };
    }
    return ops(path);
  };
  await loadInbox();
  assert.equal(requested.filter((path) => path === "/api/signposts").length, 1);
  assert.match(el.inbox.innerHTML, /Nothing waiting on a person right now/);
  for (const id of ["dash-signposts", "inbox-signposts"]) {
    assert.equal(el[id].hidden, false);
    assert.match(el[id].innerHTML, /Scenario observations/);
    assert.match(el[id].innerHTML, /&lt;slow&gt;/);
    assert.match(el[id].innerHTML, /&lt;script&gt;/);
    assert.doesNotMatch(el[id].innerHTML, /data-action|data-task|<script>/);
  }
  globalThis.fetch = async (path) => {
    if (path === "/api/signposts") throw new Error("fact unavailable");
    return ops(path);
  };
  await loadInbox();
  assert.match(el["dash-signposts"].innerHTML, /observations are unavailable/);
  assert.match(el.inbox.innerHTML, /Nothing waiting on a person right now/);
});

// --------------------------------------------------------------- dialogs

/// Just enough document for `scrim` and the dialog's own fields: every id
/// the dialog asks for is a stub that remembers its value and handlers.
function stubDialogPage() {
  const elements = {};
  const el = () => ({ value: "", textContent: "", disabled: false, hidden: false, focus() {}, classList: { add() {} } });
  globalThis.document = {
    ...bare,
    body: { appendChild() {} },
    createElement: () => ({ set innerHTML(_) {}, onclick: null }),
    querySelectorAll: () => [],
    getElementById: (id) => (elements[id] ||= el()),
  };
  return elements;
}

test("an answer cannot be sent twice while the first is still on its way", async () => {
  const el = stubDialogPage();
  let calls = 0;
  let fail;
  globalThis.fetch = () => {
    calls += 1;
    return new Promise((_, reject) => { fail = reject; });
  };
  openActionDialog("answer", { task_id: "t", run_id: "r1", title: "needs a key", reason: "which key?", actions: ["answer"] });
  const go = el["oa-confirm"];
  assert.equal(go.disabled, true, "nothing typed yet");
  el["oa-text"].value = "use the staging key";
  el["oa-reason"].value = "it asked";
  el["oa-reason"].oninput();
  assert.equal(go.disabled, false);

  const first = go.onclick();
  go.onclick();
  el["oa-text"].oninput(); // typing while it is out must not re-arm the button
  assert.equal(calls, 1, "one request, however often it is pressed");
  assert.equal(go.disabled, true, "held off while the request is out");

  fail(new Error("run r1 has no session to answer into"));
  await first;
  assert.equal(el["oa-err"].textContent, "run r1 has no session to answer into", "the daemon's refusal, in the dialog");
  assert.equal(go.disabled, false, "a refusal gives the button back");
});
