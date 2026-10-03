import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  STAGES,
  actionReady,
  actionRequest,
  agingByScope,
  agingGeometry,
  attentionRows,
  blockedWorkflowNodes,
  bulkOffers,
  bulkPreview,
  capacityText,
  cfdGeometry,
  entryKindLabel,
  entryTone,
  exceptionKey,
  failKindRows,
  figureText,
  flowBars,
  fmtAge,
  inboxItems,
  jitter,
  kindLabel,
  logScale,
  multipleGeometry,
  MULTIPLES,
  readSeen,
  seenKey,
  serverNow,
  ageNow,
  later,
  scatterGeometry,
  scheduleActions,
  scheduleRows,
  seenSnapshot,
  sinceLastLooked,
  spreadLabels,
  trend,
  utcStamp,
} from "../js/operations-model.js";

// A real `GET /api/operations?window=7d` answer, captured from a throwaway
// daemon (three scopes, alpha > beta and gamma, seeded with sixty days of
// runs; the herdr binary pointed at /usr/bin/false so nothing could reach a
// live session). Trimmed: one exception per kind, three finished runs.
const REPORT = JSON.parse(readFileSync(new URL("./fixtures/operations-report.json", import.meta.url), "utf8"));
const all = () => true;

// ------------------------------------------------------------ attention

test("attention keeps the daemon's order and carries ages forward from when the answer arrived", () => {
  const rows = attentionRows(REPORT, 60);
  assert.deepEqual(rows.map((r) => r.kind), ["aging", "failed_exhausted", "blocked", "schedule_missed"]);
  const blocked = rows.find((r) => r.kind === "blocked");
  assert.equal(blocked.age, blocked.age_s + 60, "a minute older than when the daemon answered, by the daemon's own clock");
  assert.equal(blocked.tone, "fault");
  assert.equal(rows.find((r) => r.kind === "schedule_missed").tone, "wait");
  assert.equal(blocked.label, "blocked");
});

test("ages never read the browser's clock against the server's timestamps", () => {
  // The same answer, read on a browser whose clock is a day off: nothing
  // but the elapsed time since arrival moves an age.
  const rows = attentionRows(REPORT, 0);
  for (const r of rows) assert.equal(r.age, r.age_s);
  assert.equal(ageNow(100, -5), 100, "a negative elapsed time (clock stepped back) never makes it younger");
  assert.equal(serverNow("2026-09-25T12:00:00Z", 90), "2026-09-25T12:01:30.000Z");
});

test("the Inbox is every exception a person must act on, observations left out", () => {
  const withSignpost = { attention: [...REPORT.attention, { kind: "triggered_signpost", severity: "low", since: REPORT.generated_at, age_s: 0, reason: "r", actions: [], observation: true }] };
  const items = inboxItems(withSignpost, 0);
  assert.equal(items.length, REPORT.attention.length);
  assert.ok(items.every((i) => !i.observation));
  assert.deepEqual(inboxItems(null, 0), []);
});

test("an unknown kind reads as its own id, spaced, not as nothing", () => {
  assert.equal(kindLabel("suspected_stuck"), "may be stuck");
  assert.equal(kindLabel("brand_new_kind"), "brand new kind");
});

// ------------------------------------------------- since you last looked

test("since you last looked counts new, resolved and finished against the stored snapshot", () => {
  const rows = attentionRows(REPORT, 0);
  const earlier = { at: "2026-09-25T12:00:00.000Z", keys: [rows[0].key, "aging|gone-run|2026-09-25T09:00:00Z"], last_exception_at: null };
  const finished = [{ ended_at: "2026-09-25T12:30:00Z" }, { ended_at: "2026-09-25T11:00:00Z" }];
  const d = sinceLastLooked(earlier, rows, finished, REPORT.health.current.window.from);
  // Compared as instants: a stamp with an offset and no fraction is the
  // same moment as its Z twin, and text order would say otherwise.
  assert.equal(sinceLastLooked({ ...earlier, at: "2026-09-25T14:29:59+02:00" }, rows, [{ ended_at: "2026-09-25T12:30:00.5Z" }], null).finished, 1);
  assert.equal(later("2026-09-25T12:00:00.9Z", "2026-09-25T12:00:00.10Z"), true);
  assert.equal(d.first, false);
  assert.equal(d.fresh, rows.length - 1);
  assert.equal(d.resolved, 1);
  assert.equal(d.finished, 1);
  assert.equal(d.beyondWindow, false);
  assert.equal(sinceLastLooked({ ...earlier, at: "2026-09-01T00:00:00Z" }, rows, [], REPORT.health.current.window.from).beyondWindow, true);
  assert.deepEqual(sinceLastLooked(null, rows, [], null), { first: true });
});

test("the snapshot remembers the newest exception, and keeps the last one when the queue is empty", () => {
  const rows = attentionRows(REPORT, 0);
  const snap = seenSnapshot(rows, REPORT.generated_at, null);
  assert.equal(snap.keys.length, rows.length);
  assert.equal(snap.last_exception_at, "2026-09-25T12:08:15.983734Z");
  const quiet = seenSnapshot([], serverNow(REPORT.generated_at, 1), snap);
  assert.equal(quiet.last_exception_at, snap.last_exception_at, "an empty queue carries no dates, so the old one stays");
  assert.deepEqual(readSeen(JSON.stringify(snap)), snap);
  assert.equal(readSeen("not json"), null);
  assert.equal(readSeen(JSON.stringify({ at: 5 })), null);
  assert.equal(readSeen(null), null);
});

test("each rail selection keeps its own snapshot", () => {
  assert.equal(seenKey(null), "factory-ops-seen:*");
  assert.equal(seenKey("alpha"), "factory-ops-seen:alpha");
});

test("an exception's key is its kind, its subject and when it began", () => {
  assert.equal(exceptionKey({ kind: "blocked", run_id: "r1", task_id: "t", since: "S" }), "blocked|r1|S");
  assert.equal(exceptionKey({ kind: "liveness_lost", agent: "foreman", since: "S" }), "liveness_lost|foreman|S");
});

// ---------------------------------------------------------------- actions

test("each action goes to its slice-2 route with the reason in the body", () => {
  const row = { task_id: "t 1", run_id: "r1", next_run_at: "2026-09-26T00:00:00Z" };
  assert.deepEqual(actionRequest("run_again", row, { reason: "  " }), { path: "/api/tasks/t%201/run", method: "POST", body: {} });
  assert.deepEqual(actionRequest("run_now", row, { reason: "ahead" }).body, { reason: "ahead" });
  assert.deepEqual(actionRequest("cancel", row, { reason: "stuck" }), { path: "/api/tasks/t%201/cancel", method: "POST", body: { reason: "stuck", run_id: "r1" } },
    "the attempt the person was shown, so a retry started since is not the one ended");
  assert.deepEqual(actionRequest("cancel", { task_id: "t" }, {}).body, {});
  assert.deepEqual(actionRequest("skip_next", row, {}), { path: "/api/tasks/t%201/skip-next", method: "POST", body: { slot: "2026-09-26T00:00:00Z" } },
    "the slot the person was shown, so a schedule that moved on is refused");
  assert.deepEqual(actionRequest("skip_next", { task_id: "t" }, { reason: "x" }).body, { reason: "x" });
  assert.deepEqual(actionRequest("pause_schedule", row, { reason: "freeze" }), { path: "/api/tasks/t%201", method: "PATCH", body: { schedule_paused: true, reason: "freeze" } });
  assert.deepEqual(actionRequest("resume_schedule", row, {}).body, { schedule_paused: false });
  assert.deepEqual(actionRequest("answer", row, { text: "yes", reason: "asked" }), { path: "/api/runs/r1/answer", method: "POST", body: { text: "yes", reason: "asked" } });
  assert.deepEqual(actionRequest("approve", row, { reason: "release owner checked it" }), { path: "/api/runs/r1/approve", method: "POST", body: { reason: "release owner checked it" } });
  assert.deepEqual(actionRequest("reject", row, { reason: "missing evidence" }), { path: "/api/runs/r1/reject", method: "POST", body: { reason: "missing evidence" } });
  assert.deepEqual(actionRequest("accept_rework", row), { path: "/api/runs/r1/rework", method: "POST", body: {} });
  assert.equal(actionRequest("nonsense", row, {}), null);
});

test("an answer needs both its text and a reason before it can be confirmed; nothing else does", () => {
  assert.equal(actionReady("answer", { text: "yes", reason: "" }), false);
  assert.equal(actionReady("answer", { text: " ", reason: "why" }), false);
  assert.equal(actionReady("answer", { text: "yes", reason: "why" }), true);
  assert.equal(actionReady("approve", { reason: "" }), false);
  assert.equal(actionReady("reject", { reason: "concrete finding" }), true);
  assert.equal(actionReady("accept_rework", {}), true);
  assert.equal(actionReady("cancel", {}), true);
});

test("a bulk preview lists every row that offers the action, once per task", () => {
  const rows = attentionRows(REPORT, 0);
  const cancel = bulkPreview(rows, "cancel");
  assert.equal(cancel.count, 2);
  assert.deepEqual(cancel.rows.map((r) => r.title), ["tidy stale branches", "summarise support inbox"]);
  const twice = [...rows, { ...rows[0], kind: "suspected_stuck", key: "other" }];
  assert.equal(bulkPreview(twice, "cancel").count, 2, "two exceptions of one task still cancel it once");
  // Only offered when there is more than one row to act on.
  assert.deepEqual(bulkOffers(rows).map((p) => p.action), ["cancel"]);
});

// ------------------------------------------------------------------- flow

test("flow bars share one scale and capacity reads unknown, never zero", () => {
  const bars = flowBars(REPORT.flow, all);
  const alpha = bars.find((b) => b.scope === "alpha");
  assert.equal(alpha.total, 2);
  assert.deepEqual(alpha.segments.map((s) => s.stage), ["running", "blocked"]);
  assert.equal(alpha.segments[1].x, alpha.segments[0].w, "stacked end to end");
  const widest = Math.max(...bars.map((b) => b.segments.reduce((w, s) => w + s.w, 0)));
  assert.equal(widest, 100);
  assert.equal(capacityText(alpha), "2 in use · capacity unknown");
  assert.equal(capacityText({ sessions_in_use: 3, sessions_max: 4 }), "3 of 4 · 75%");
  assert.equal(figureText({ value: null, samples: 0, reason: "no data" }, fmtAge), "—");
  assert.equal(figureText(alpha.wait_p50, fmtAge), "42s");
});

// ------------------------------------------------------------ aging WIP

test("aging groups by scope, each with its own lines or none", () => {
  const groups = agingByScope(REPORT.aging, all);
  assert.deepEqual(groups.map((g) => g.scope).sort(), ["alpha", "beta", "gamma"]);
  assert.ok(groups.every((g) => g.lines && g.lines.samples >= 5));
  assert.equal(agingByScope({ items: [{ scope: "x", stage: "running", age_s: 5, task_id: "t", basis: "not_enough_history" }], percentiles: [{ scope: "x" }] }, all)[0].lines, null);
});

test("the aging chart puts a dot in its stage's column and higher when older", () => {
  const g = agingByScope(REPORT.aging, all).find((x) => x.scope === "alpha");
  const geo = agingGeometry(g.items, g.lines, 0, { width: 100, height: 240 });
  const colW = 100 / STAGES.length;
  for (const d of geo.dots) {
    const col = STAGES.indexOf(d.stage);
    assert.ok(d.cx >= col * colW && d.cx <= (col + 1) * colW, `${d.title} stays in its column`);
    assert.equal(d.paceClass, `pace-${d.pace}`);
  }
  const [older, younger] = [...geo.dots].sort((a, b) => b.age - a.age);
  assert.ok(older.cy < younger.cy, "older is higher up");
  assert.deepEqual(geo.lines.map((l) => l.name), ["p50", "p70", "p85", "p95"]);
  assert.ok(geo.lines[0].y > geo.lines[3].y, "p95 above p50");
  // A report a minute old draws its dots a minute older.
  const later = agingGeometry(g.items, g.lines, 60, { width: 100, height: 240 });
  assert.equal(later.dots[0].age, geo.dots[0].age + 60);
  assert.equal(jitter("a"), jitter("a"), "a dot keeps its place across re-renders");
});

test("the log scale floors at ten seconds and hugs the data", () => {
  const s = logScale([3600, 7200], 200);
  assert.ok(s.floor >= 600, "hour-long runs do not spend the chart on seconds");
  assert.ok(s.ticks.some((t) => t.label === "1h"));
  const tiny = logScale([0, 2], 200);
  assert.equal(tiny.floor, 10);
  assert.equal(tiny.y(0), tiny.y(10), "zero sits on the floor, not at -infinity");
});

test("percentile labels are spread apart and stay near their lines", () => {
  const ys = [100, 98, 97, 60];
  const out = spreadLabels(ys, 11);
  const sorted = [...out].sort((a, b) => a - b);
  for (let i = 1; i < sorted.length; i++) assert.ok(sorted[i] - sorted[i - 1] >= 10.999);
  const mean = (xs) => xs.reduce((a, b) => a + b, 0) / xs.length;
  assert.ok(Math.abs(mean(out) - mean(ys)) < 1e-9, "the block is centred on the lines, not pushed only downward");
  assert.deepEqual(spreadLabels([10, 50], 11), [10, 50], "already apart: untouched");
});

// ------------------------------------------------------------------ health

test("small multiples draw both windows, and a step with nothing finished is a gap", () => {
  const { current, previous } = REPORT.health;
  assert.equal(current.days.length, 7);
  const rate = MULTIPLES.find((m) => m.id === "scrap");
  const days = [{ finished: 2, scrapped: 1 }, { finished: 0, scrapped: 0 }, { finished: 4, scrapped: 0 }];
  const g = multipleGeometry(days, days, rate.of, { width: 100, height: 44 });
  assert.equal(g.current.length, 2, "split at the empty step");
  assert.equal(g.previous.length, 2);
  const tp = multipleGeometry(current.days, previous.days, MULTIPLES[0].of);
  assert.equal(tp.empty, false);
  assert.equal(multipleGeometry([], [], MULTIPLES[0].of).empty, true);
});

test("a trend is judged by which way is better, and noise is flat", () => {
  assert.deepEqual(trend({ value: 0.2 }, { value: 0.1 }, "lower"), { dir: "up", tone: "down" });
  assert.deepEqual(trend({ value: 5 }, { value: 4 }, "higher"), { dir: "up", tone: "up" });
  assert.equal(trend({ value: 1.0 }, { value: 1.02 }, "higher").dir, "flat");
  assert.equal(trend({ value: null }, { value: 1 }, "higher").dir, "none");
});

test("fail kinds come largest first, with unclassified kept apart", () => {
  const rows = failKindRows({ fail_by_kind: { run_timeout: 1, agent_failed: 3 }, unclassified: 2 });
  assert.deepEqual(rows.map((r) => r.kind), ["agent_failed", "run_timeout", "unclassified"]);
  assert.equal(rows[0].w, 100);
  assert.equal(rows[0].label, "agent reported failure");
  assert.deepEqual(failKindRows({ fail_by_kind: {}, unclassified: 0 }), []);
});

test("the scatter draws only runs that ended done, placed by when they ended", () => {
  const w = { from: "2026-09-18T00:00:00Z", to: "2026-09-25T00:00:00Z" };
  const runs = [
    { run_id: "a", task_id: "t", ended_at: "2026-09-24T12:00:00Z", status: "done", cycle_s: 600 },
    { run_id: "b", task_id: "t", ended_at: "2026-09-19T00:00:00Z", status: "failed" },
  ];
  const g = scatterGeometry(runs, w, { cycle_p50: { value: 600 }, cycle_p85: { value: null } }, { width: 100, height: 200 });
  assert.equal(g.dots.length, 1);
  assert.ok(Math.abs(g.dots[0].cx - (6.5 / 7) * 100) < 1e-9);
  assert.deepEqual(g.lines.map((l) => l.name), ["p50"], "an unknown p85 draws no line");
  assert.equal(scatterGeometry(REPORT.health.current.finished_runs.filter((r) => !r.cycle_s), w, {}).empty, true);
});

test("the CFD stacks cumulative finished, then in progress, then waiting", () => {
  const days = [
    { to: "d1", finished: 2, in_progress: 1, waiting: 0 },
    { to: "d2", finished: 3, in_progress: 2, waiting: 1 },
  ];
  const g = cfdGeometry(days, { width: 100, height: 100 });
  assert.deepEqual(g.stacks.map((s) => [s.done, s.progress, s.waiting]), [[2, 3, 3], [5, 7, 8]]);
  assert.equal(g.max, 8);
  assert.deepEqual(g.bands.map((b) => b.id), ["done", "progress", "waiting"]);
  assert.equal(cfdGeometry([], {}).empty, true);
  assert.equal(cfdGeometry(REPORT.health.current.days).stacks.length, 7);
});

// --------------------------------------------------------------- schedules

test("schedules sort late, missed, paused, then by next slot, with the zone named", () => {
  const rows = scheduleRows(REPORT.schedules, all);
  assert.deepEqual(rows.map((r) => r.state), ["missed", "paused", "due", "due"]);
  assert.equal(rows[0].zone, "Europe/Berlin");
  assert.equal(rows.find((r) => r.schedule === "every 3600s").zone, "UTC");
  assert.equal(rows[0].next, "2026-09-26 00:00 UTC");
  assert.deepEqual(scheduleActions({ state: "paused" }), ["resume_schedule"]);
  assert.deepEqual(scheduleActions({ state: "late" }), ["run_now", "skip_next", "pause_schedule"]);
});

test("blocked workflow nodes are read off the task list and narrowed by the rail", () => {
  const tasks = [
    { id: "a", status: "blocked", scope: "alpha", updated_at: "2", workflow_origin: { workflow_id: "w", node_id: "n", workflow_run_id: "r" } },
    { id: "b", status: "blocked", scope: "alpha", updated_at: "1" },
    { id: "c", status: "running", scope: "alpha", updated_at: "0", workflow_origin: {} },
    { id: "d", status: "blocked", scope: "gamma", updated_at: "0", workflow_origin: {} },
  ];
  assert.deepEqual(blockedWorkflowNodes(tasks, (s) => s === "alpha").map((t) => t.id), ["a"]);
});

// ------------------------------------------------------------------ misc

test("times read the same everywhere: fixed UTC stamps and short ages", () => {
  assert.equal(utcStamp("2026-09-25T09:05:00Z"), "2026-09-25 09:05 UTC");
  assert.equal(utcStamp("nope"), "—");
  assert.equal(fmtAge(45), "45s");
  assert.equal(fmtAge(12 * 60), "12m");
  assert.equal(fmtAge(3.2 * 3600), "3.2h");
  assert.equal(fmtAge(4 * 86400), "4.0d");
  assert.equal(fmtAge(null), "—");
});

test("the journal names the new kinds and marks who asked", () => {
  assert.equal(entryKindLabel("slot_skipped"), "slot skipped");
  assert.equal(entryKindLabel("answer_unsent"), "answer not sent");
  assert.equal(entryKindLabel("progress"), "progress");
  assert.equal(entryTone({ kind: "run_requested" }), "ask");
  assert.equal(entryTone({ kind: "schedule_skipped" }), "missed");
  assert.equal(entryTone({ kind: "answer_unsent" }), "fault");
  assert.equal(entryTone({ kind: "blocked" }), "");
});

test("a harness that does not start has its own label and journal lines (#131)", () => {
  assert.equal(kindLabel("harness_unhealthy"), "harness does not start");
  assert.equal(entryKindLabel("harness_unhealthy"), "harness does not start");
  assert.equal(entryKindLabel("harness_recovered"), "harness answers again");
  assert.equal(entryTone({ kind: "harness_unhealthy" }), "fault");
  const rows = attentionRows({
    attention: [
      { kind: "harness_unhealthy", severity: "high", agent: "codex", title: "/opt/homebrew/bin/codex", since: "2026-09-25T12:00:00Z", age_s: 60, reason: "codex does not start", actions: [] },
    ],
  });
  assert.equal(rows[0].label, "harness does not start");
  assert.equal(rows[0].key, "harness_unhealthy|codex|2026-09-25T12:00:00Z");
});
