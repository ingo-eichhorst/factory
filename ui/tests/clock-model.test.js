import test from "node:test";
import assert from "node:assert/strict";

import {
  DUE_SOON_SECONDS,
  clockIndex,
  clockRows,
  deadlineLabel,
  deadlineRow,
  dueText,
  fmtSpan,
  inboxClockRows,
  itemKey,
  itemRow,
  nowFromClock,
  reportDeadlines,
  stateLabel,
} from "../js/clock-model.js";

const FINDING = { kind: "finding", scope: "demo", vulnerability: "CVE-2026-1234" };
const REPORT = { kind: "report", item: "task-1" };

// --------------------------------------------------------------- vocabulary

test("itemKey mirrors ClockItemRef's Display, the CLI's own text form", () => {
  assert.equal(itemKey(FINDING), "finding:demo:CVE-2026-1234");
  assert.equal(itemKey(REPORT), "report:task-1");
});

test("deadlineLabel/stateLabel translate the wire's snake_case, falling back to the raw id", () => {
  assert.equal(deadlineLabel("early_warning"), "24h early warning");
  assert.equal(deadlineLabel("notification"), "72h notification");
  assert.equal(deadlineLabel("bogus"), "bogus");
  assert.equal(stateLabel("due"), "due");
  assert.equal(stateLabel("overdue"), "overdue");
  assert.equal(stateLabel("met"), "met");
  assert.equal(stateLabel("late"), "met late");
  assert.equal(stateLabel("bogus"), "bogus");
});

test("fmtSpan is the same short-duration idiom as operations-model.js's fmtAge", () => {
  assert.equal(fmtSpan(45), "45s");
  assert.equal(fmtSpan(90), "2m");
  assert.equal(fmtSpan(3600), "60m", "under 90 minutes stays in minutes, same as fmtAge");
  assert.equal(fmtSpan(3 * 3600), "3.0h");
  assert.equal(fmtSpan(10 * 3600), "10h");
  assert.equal(fmtSpan(2 * 86400), "2.0d");
  assert.equal(fmtSpan(-5), "0s", "never a negative span");
});

// -------------------------------------------------------------------- time

test("nowFromClock advances the clock's own now by the elapsed seconds since it arrived", () => {
  const clock = { now: "2026-09-24T10:00:00Z" };
  assert.equal(nowFromClock(clock, 0), "2026-09-24T10:00:00.000Z");
  assert.equal(nowFromClock(clock, 90), "2026-09-24T10:01:30.000Z");
  assert.equal(nowFromClock(clock, -5), "2026-09-24T10:00:00.000Z", "never a negative elapsed");
});

test("dueText: due in, overdue by, met with time to spare, met late", () => {
  const dueAt = "2026-09-24T12:00:00Z";
  assert.equal(dueText({ due_at: dueAt, submission: null }, "2026-09-24T09:00:00Z"), "due in 3.0h");
  assert.equal(dueText({ due_at: dueAt, submission: null }, "2026-09-24T14:00:00Z"), "overdue by 2.0h");
  assert.equal(
    dueText({ due_at: dueAt, submission: { at: "2026-09-24T09:30:00Z" } }, "2026-09-24T14:00:00Z"),
    "met, 2.5h to spare",
  );
  assert.equal(
    dueText({ due_at: dueAt, submission: { at: "2026-09-24T12:30:00Z" } }, "2026-09-24T14:00:00Z"),
    "met, 30m late",
  );
});

// -------------------------------------------------------------------- rows

test("deadlineRow/itemRow/clockRows shape the wire, and clockRows degrades to [] with no clock", () => {
  const deadline = { deadline: "early_warning", due_at: "2026-09-24T12:00:00Z", state: "overdue", submission: null };
  const row = deadlineRow(deadline, "2026-09-24T13:00:00Z");
  assert.equal(row.label, "24h early warning");
  assert.equal(row.stateLabel, "overdue");
  assert.equal(row.text, "overdue by 60m");
  assert.equal(row.submission, null);

  const item = {
    item: FINDING,
    scope: "demo",
    awareness_at: "2026-09-23T12:00:00Z",
    excluded: null,
    reported_now: true,
    deadlines: [deadline],
  };
  const shaped = itemRow(item, "2026-09-24T13:00:00Z");
  assert.equal(shaped.key, "finding:demo:CVE-2026-1234");
  assert.equal(shaped.kind, "finding");
  assert.deepEqual(shaped.ref, FINDING);
  assert.equal(shaped.deadlines.length, 1);

  assert.deepEqual(clockRows(null, "2026-09-24T13:00:00Z"), []);
  assert.deepEqual(clockRows(undefined, "2026-09-24T13:00:00Z"), []);
  assert.equal(clockRows({ items: [item] }, "2026-09-24T13:00:00Z").length, 1);
});

test("an excluded finding carries no deadlines, and itemRow says why", () => {
  const item = {
    item: FINDING,
    scope: "demo",
    awareness_at: "2026-09-23T12:00:00Z",
    excluded: "not_affected",
    reported_now: true,
    deadlines: [],
  };
  const shaped = itemRow(item, "2026-09-24T13:00:00Z");
  assert.equal(shaped.excluded, "not_affected");
  assert.deepEqual(shaped.deadlines, []);
});

// ------------------------------------------------------------------ intake

test("reportDeadlines finds a confirmed report directly, or one hop up through a split's parent", () => {
  const overdue = { deadline: "early_warning", due_at: "2026-09-24T12:00:00Z", state: "overdue", submission: null };
  const rootItem = { item: { kind: "report", item: "root-id" }, scope: "web", awareness_at: "x", excluded: null, reported_now: true, deadlines: [overdue] };
  const idx = clockIndex({ now: "2026-09-24T13:00:00Z", items: [rootItem] });

  const direct = reportDeadlines(idx, { id: "root-id" }, "2026-09-24T13:00:00Z");
  assert.equal(direct.key, "report:root-id");

  const child = reportDeadlines(idx, { id: "child-id", parent: "root-id" }, "2026-09-24T13:00:00Z");
  assert.equal(child.key, "report:root-id", "a split part resolves one hop up to its parent");

  assert.equal(reportDeadlines(idx, { id: "grandchild-id", parent: "child-id" }, "2026-09-24T13:00:00Z"), null,
    "two hops from the root is out of scope -- resolved server-side only, never walked again here");
  assert.equal(reportDeadlines(idx, { id: "unrelated-id" }, "2026-09-24T13:00:00Z"), null);
  assert.equal(reportDeadlines(null, { id: "root-id" }, "2026-09-24T13:00:00Z"), null);
  assert.equal(reportDeadlines(idx, null, "2026-09-24T13:00:00Z"), null);
});

// ------------------------------------------------------------------- inbox

function findingItem(vuln, dueAt, state, extra = {}) {
  return {
    item: { kind: "finding", scope: "demo", vulnerability: vuln },
    scope: "demo",
    awareness_at: "2026-09-23T12:00:00Z",
    excluded: null,
    reported_now: true,
    deadlines: [{ deadline: "early_warning", due_at: dueAt, state, submission: null, ...extra }],
  };
}

function reportItem(id, dueAt, state, extra = {}) {
  return {
    item: { kind: "report", item: id },
    scope: "web",
    awareness_at: "2026-09-23T12:00:00Z",
    excluded: null,
    reported_now: true,
    deadlines: [{ deadline: "notification", due_at: dueAt, state, submission: null, ...extra }],
  };
}

test("inboxClockRows shows only overdue and due-soon deadlines, never met/late or excluded", () => {
  const now = "2026-09-24T12:00:00Z";
  const clock = {
    now,
    items: [
      findingItem("CVE-overdue", "2026-09-24T10:00:00Z", "overdue"), // 2h overdue
      findingItem("CVE-soon", "2026-09-24T14:00:00Z", "due"), // due in 2h -- inside the 6h window
      findingItem("CVE-later", "2026-09-25T20:00:00Z", "due"), // due in 32h -- outside the window
      findingItem("CVE-met", "2026-09-24T14:00:00Z", "met", { submission: { at: "2026-09-24T09:00:00Z" } }),
      { ...findingItem("CVE-excluded", "2026-09-24T14:00:00Z", "overdue"), excluded: "not_affected" },
      reportItem("task-1", "2026-09-24T09:00:00Z", "overdue"), // 3h overdue
    ],
  };
  const titleOf = (id) => `task ${id}`;
  const hrefOf = (scope) => `#dependencies/${scope}`;
  const rows = inboxClockRows(clock, 0, titleOf, hrefOf);

  assert.deepEqual(
    rows.map((r) => r.title),
    ["task task-1", "CVE-overdue", "CVE-soon"],
    "overdue first (longest overdue first), then due-soon, excluded/met/too-far-out left out",
  );
  assert.equal(rows[0].tone, "fault");
  assert.equal(rows[0].task_id, "task-1");
  assert.equal(rows[0].href, null);
  assert.equal(rows[2].tone, "wait");
  assert.equal(rows[2].task_id, null);
  assert.equal(rows[2].href, "#dependencies/demo");
  assert.match(rows[2].reason, /due in/);
  assert.match(rows[0].reason, /overdue by/);
});

test("inboxClockRows degrades to [] with no clock", () => {
  assert.deepEqual(inboxClockRows(null, 0, () => "", () => ""), []);
  assert.deepEqual(inboxClockRows(undefined, 0, () => "", () => ""), []);
});

test("DUE_SOON_SECONDS is six hours, the window a due (not yet overdue) deadline has to fall inside to show up", () => {
  assert.equal(DUE_SOON_SECONDS, 6 * 3600);
});
