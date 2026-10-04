import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  CLOSED, columnFor, standing, taskActions, closeReason, closeBody, blockedByFailure, isSettled, failKindLabel, relationLabel,
} from "../js/task-model.js";
import { attemptState } from "../js/bench-model.js";
import { cardNote, cardActions } from "../js/intake-model.js";
import { entryKindLabel, entryTone } from "../js/operations-model.js";

const failure = (kind, attempt = 1) => ({ kind, run_id: "r1", attempt, at: "2026-09-25T10:00:00Z" });
const FAIL_KINDS = [
  "agent_failed", "session_gone", "turn_ended", "stop_failure",
  "ack_timeout", "run_timeout", "blocked_timeout", "dispatch_failed",
];

test("an upstream wait is scheduled even without a cron and cannot be run accidentally", () => {
  for (const after of [[], ["parent"]]) {
    const task = { status: "pending", after };
    assert.equal(columnFor(task), "scheduled");
    assert.equal(taskActions(task, null).run, false);
    assert.equal(taskActions(task, null).close, true);
  }
  assert.equal(taskActions({ status: "pending", after: null }, null).run, true);
});

test("a task whose run failed sits in Blocked, never Closed, for every fail kind", () => {
  for (const kind of FAIL_KINDS) {
    const t = { status: "blocked", failure: failure(kind), error: "boom\nand more" };
    assert.equal(columnFor(t), "blocked", kind);
    assert.ok(blockedByFailure(t));
    const s = standing(t);
    assert.equal(s.tone, "fault");
    assert.match(s.text, /^attempt 1 failed: /);
    assert.equal(s.text, `attempt 1 failed: ${failKindLabel(kind)}`);
    assert.equal(s.detail, "boom\nand more", "the card shows the last error");
  }
  // A row stored as failed before the daemon moved it is still not closed.
  assert.equal(columnFor({ status: "failed" }), "blocked");
  assert.equal(standing({ status: "failed", error: "old" }).text, "last attempt failed: unclassified");
});

test("a task blocked on a question reads differently from one blocked by a failure", () => {
  const asking = { status: "blocked" };
  assert.equal(columnFor(asking), "blocked");
  assert.equal(blockedByFailure(asking), false);
  assert.equal(standing(asking), null);
  assert.equal(isSettled(asking), false, "its run is still waiting");
});

test("Closed holds only done and deliberately closed tasks, each with its reason", () => {
  assert.deepEqual(CLOSED, ["done", "cancelled"]);
  assert.equal(columnFor({ status: "done" }), "closed");
  assert.equal(standing({ status: "done" }).text, "completed");
  assert.equal(standing({ status: "cancelled" }).text, "won't do", "a cancelled run closes as not planned");
  const dup = { status: "cancelled", closure: { reason: "duplicate", duplicate_of: "abcdef1234", note: "same bug", by: "the owner" } };
  assert.equal(closeReason(dup), "duplicate");
  assert.deepEqual(standing(dup), { tone: "closed", text: "duplicate of abcdef12", detail: "same bug" });
  const wont = { status: "cancelled", closure: { reason: "not_planned", by: "the owner" }, failure: failure("agent_failed") };
  assert.equal(columnFor(wont), "closed");
  assert.equal(standing(wont).text, "won't do", "closed on purpose: the close wins over the old failure");
});

test("a routed workflow task reads done with its selected target", () => {
  assert.deepEqual(standing({ status: "done", routed_to: "implement" }), {
    tone: "closed", text: "done → implement", detail: null,
  });
});

test("a scheduled task mid-retry stays in Scheduled and says it is retrying", () => {
  const t = { status: "pending", schedule: { every: { seconds: 60 } }, pending_retry: { attempts: 2 }, failure: failure("run_timeout") };
  assert.equal(columnFor(t), "scheduled");
  assert.deepEqual(standing(t), { tone: "wait", text: "retrying after ran past its timeout (retry 2)", detail: null });
  // A failure with no queued retry is not a retry.
  assert.equal(standing({ status: "pending", failure: failure("run_timeout") }), null);
  // Retries exhausted: blocked, never a healthy-looking scheduled card.
  assert.equal(columnFor({ status: "blocked", schedule: { every: { seconds: 60 } }, failure: failure("run_timeout") }), "blocked");
});

test("a decomposition child waits in Scheduled and names its real parent relation", () => {
  const child = {
    status: "pending",
    parent_task_id: "parent-123456",
    decomposition_part: "surface",
    depends_on: ["foundation-id"],
  };
  assert.equal(columnFor(child), "scheduled");
  assert.deepEqual(standing(child), {
    tone: "wait", text: "waiting for 1 prerequisite", detail: null,
  });
  const tasks = new Map([["parent-123456", { title: "Build the subsystem" }]]);
  assert.equal(relationLabel(child, tasks), "part surface of Build the subsystem");
});

test("close works with no active run, reopen only on a closed task", () => {
  const failed = { status: "blocked", failure: failure("agent_failed") };
  assert.deepEqual(taskActions(failed, null), { run: true, cancel: false, close: true, reopen: false });
  assert.deepEqual(taskActions({ status: "pending" }, null), { run: true, cancel: false, close: true, reopen: false });
  assert.deepEqual(taskActions({ status: "running" }, { id: "r" }), { run: false, cancel: true, close: false, reopen: false });
  assert.deepEqual(taskActions({ status: "done" }, null), { run: true, cancel: false, close: false, reopen: true });
  assert.equal(taskActions({ status: "intake" }, null).close, false, "intake has its own wontfix");
  assert.equal(taskActions({ status: "cancelled", intake: { stage: "wontfix" } }, null).reopen, false);
});

test("the close body carries duplicate_of only with duplicate, and no empty note", () => {
  assert.deepEqual(closeBody("not_planned", "t0", "  "), { reason: "not_planned" });
  assert.deepEqual(closeBody("duplicate", " t0 ", "same stack"), { reason: "duplicate", duplicate_of: "t0", note: "same stack" });
  assert.deepEqual(closeBody("completed", "", "done by hand"), { reason: "completed", note: "done by hand" });
});

test("a bench attempt whose task is blocked by a failure is judging, not running", () => {
  const attempt = { verdict: null, task_id: "t1" };
  assert.equal(attemptState(attempt, { status: "blocked", failure: failure("dispatch_failed") }), "judging");
  assert.equal(attemptState(attempt, { status: "blocked" }), "running");
  assert.equal(attemptState(attempt, { status: "done" }), "judging");
});

test("an intake card whose triage task failed is back to be triaged again", () => {
  const card = { stage: "triaging", triage_task: "t9", triage: null, triage_task_status: "blocked", triage_task_ended: true };
  assert.match(cardNote(card), /ended \(failed\) without an assessment/);
  assert.ok(cardActions(card).includes("triage"));
  const asking = { ...card, triage_task_ended: false };
  assert.equal(cardNote(asking), "triage run blocked");
  assert.ok(!cardActions(asking).includes("triage"));
});

test("the journal names a close, a reopen and a failure block", () => {
  assert.equal(entryKindLabel("closed"), "closed");
  assert.equal(entryKindLabel("reopened"), "reopened");
  assert.equal(entryTone({ kind: "closed" }), "ask");
  assert.equal(entryTone({ kind: "blocked_on_failure" }), "fault");
  assert.equal(entryTone({ kind: "migrated" }), "");
});

test("the board takes its column from task-model, and the modal offers close and reopen", () => {
  const tasks = readFileSync(new URL("../js/tasks.js", import.meta.url), "utf8");
  assert.ok(!/function columnFor/.test(tasks), "one columnFor, in task-model.js");
  assert.match(tasks, /id="m-close-task"/);
  assert.match(tasks, /id="m-reopen"/);
  assert.match(tasks, /\/api\/tasks\/\$\{state\.open\}\/close/);
});
