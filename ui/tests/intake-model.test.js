import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  COLUMNS,
  addRequest,
  agentText,
  assessRequest,
  assessmentProblem,
  axisMarks,
  buildAssessment,
  buildParts,
  cardActions,
  cardNote,
  cards,
  decideProblem,
  decideRequest,
  estimateOf,
  estimateText,
  fmtAge,
  infoRequest,
  nextActions,
  previewVerdict,
  priorityOf,
  routeFor,
  routeProblem,
  splitDraft,
  splitProblem,
  totalOpen,
  touchesIntake,
  triageRequest,
  verdictChips,
} from "../js/intake-model.js";

// Captured from a throwaway daemon's `GET /api/intake` after the `#119`
// end-to-end run: one item in each column, one closed as wontfix -- with
// the next actions, a split count and the routes added by hand in the
// daemon's shape.
const { board } = JSON.parse(readFileSync(new URL("./fixtures/intake-board.json", import.meta.url), "utf8"));
const card = (key) => cards(board, key)[0];

test("the four columns are received, triaging, needs info and ready, in that order", () => {
  assert.deepEqual(COLUMNS.map(c => c.key), ["received", "triaging", "needs_info", "ready"]);
  for (const c of COLUMNS) assert.equal(cards(board, c.key).length, 1, c.key);
  assert.equal(totalOpen(board), 3);
  assert.equal(board.wontfix, 1);
  assert.deepEqual(cards(null, "ready"), []);
});

test("the priority preview is the daemon's impact x urgency matrix", () => {
  const grid = [
    ["high", "high", "P1"], ["high", "medium", "P2"], ["medium", "high", "P2"],
    ["high", "low", "P3"], ["medium", "medium", "P3"], ["low", "high", "P3"],
    ["medium", "low", "P4"], ["low", "medium", "P4"], ["low", "low", "P4"],
  ];
  for (const [i, u, p] of grid) assert.equal(priorityOf(i, u), p, `${i} x ${u}`);
  // and it agrees with what the daemon actually stored
  for (const c of COLUMNS) {
    const t = card(c.key).triage;
    assert.equal(priorityOf(t.assessment.impact, t.assessment.urgency), t.priority);
  }
});

test("the estimate preview is ir:triage's table and spells ranges as the daemon does", () => {
  assert.equal(estimateText(estimateOf(2)), "15m-45m");
  assert.equal(estimateText(estimateOf(4)), "45m-2h");
  assert.equal(estimateText(estimateOf(6)), "1h30m-3h");
  assert.equal(estimateText(estimateOf(8)), "2h30m-5h");
  assert.equal(estimateOf(9), null);
  assert.equal(estimateText(null), "no estimate");
  assert.equal(estimateText(card("ready").triage.estimate), "45m-2h");
  assert.equal(verdictChips(card("ready")).estimate, "45m-2h");
});

test("axis marks: pass, fail, a tolerated cheap observability gap, and unassessed", () => {
  const ready = axisMarks(card("ready"), board.axes);
  assert.equal(ready.length, 7);
  assert.equal(ready.find(m => m.axis === "observability").mark, "tolerated");
  assert.equal(ready.filter(m => m.mark === "pass").length, 6);
  const needs = axisMarks(card("needs_info"), board.axes);
  assert.equal(needs.find(m => m.axis === "independence").mark, "fail");
  assert.match(needs.find(m => m.axis === "independence").evidence, /partner/);
  const fresh = axisMarks({ triage: null }, board.axes);
  assert.ok(fresh.every(m => m.mark === "unassessed"));
  assert.equal(fresh[0].pass_condition, board.axes[0].pass_condition);
});

test("actions follow the stage: a released item has none, a needs-info one takes information", () => {
  assert.deepEqual(cardActions(card("ready")), []);
  assert.deepEqual(cardActions(card("needs_info")), ["info", "triage", "assess", "split", "wontfix"]);
  assert.deepEqual(cardActions({ ...card("ready"), stage: "split" }), [], "a split item is done with");
  assert.ok(cardActions(card("triaging")).includes("release"), "assessed ready waits for release");
  const running = { stage: "triaging", triage: null, triage_task: "t", triage_task_status: "running" };
  assert.ok(!cardActions(running).includes("triage"), "one triage run at a time");
  assert.ok(!cardActions({ ...running, triage_task_status: "failed" }).includes("release"));
  assert.ok(cardActions({ ...running, triage_task_status: "failed" }).includes("triage"));
});

test("a card says what is happening to it", () => {
  assert.match(cardNote(card("triaging")), /assessed ready/);
  assert.match(cardNote(card("ready")), /^released as /);
  assert.match(
    cardNote({ stage: "triaging", triage: null, triage_task: "t", triage_task_status: "failed" }),
    /ended \(failed\) without an assessment/,
  );
  assert.equal(cardNote({ stage: "received", triage: null }), null);
});

test("an assessment is built to the wire's shape and checked the daemon's way", () => {
  const values = {
    axes: board.axes.map(({ axis }) => ({ axis, pass: axis !== "observability", evidence: ` ${axis} ok `, cost: "low" })),
    category: "bugfix", impact: "high", urgency: "medium", complexity: "4",
    scope: "demo", agent: " ", workflow: "", summary: " s ", questions: "a?\n\n b? ",
  };
  const a = buildAssessment(values);
  assert.equal(a.axes.length, 7);
  assert.equal(a.axes[0].evidence, "scope ok");
  assert.equal(a.axes.find(x => x.axis === "observability").cost, "low");
  assert.equal(a.axes.find(x => x.axis === "scope").cost, undefined, "cost only on a failed observability");
  assert.deepEqual(a.routing, { scope: "demo" });
  assert.equal(a.complexity, 4);
  assert.deepEqual(a.questions, ["a?", "b?"]);
  assert.equal(assessmentProblem(a), null);
  assert.deepEqual(previewVerdict(a), { verdict: "ready", blockers: [] });

  assert.match(assessmentProblem({ ...a, category: "Bug Fix" }), /slug/);
  assert.match(assessmentProblem({ ...a, complexity: 11 }), /1 to 10/);
  assert.match(assessmentProblem({ ...a, routing: { scope: "" } }), /scope/);
  const noCost = buildAssessment({ ...values, axes: values.axes.map(x => ({ ...x, cost: "" })) });
  assert.match(assessmentProblem(noCost), /cost/);
  const noEvidence = buildAssessment({ ...values, axes: values.axes.map((x, i) => (i === 2 ? { ...x, evidence: " " } : x)) });
  assert.match(assessmentProblem(noEvidence), /verifiability/);

  const high = buildAssessment({ ...values, axes: values.axes.map(x => ({ ...x, cost: "high" })) });
  assert.deepEqual(previewVerdict(high).blockers, ["observability"]);
  assert.deepEqual(previewVerdict({ ...a, complexity: 9 }).blockers, ["complexity 9"]);
});

test("the requests are the daemon's routes and bodies", () => {
  assert.deepEqual(addRequest({ title: " x ", instructions: "y", scope: "demo", reference: "", requester: " Kim " }), {
    path: "/api/intake", method: "POST",
    body: { title: "x", instructions: "y", source: "ui", scope: "demo", requester: "Kim" },
  });
  assert.deepEqual(triageRequest("a/b", ""), { path: "/api/intake/a%2Fb/triage", method: "POST", body: {} });
  assert.deepEqual(triageRequest("i", " pi "), { path: "/api/intake/i/triage", method: "POST", body: { agent: "pi" } });
  assert.deepEqual(infoRequest("i", "more").body, { text: "more" });
  assert.deepEqual(assessRequest("i", { axes: [] }, true).body, { assessment: { axes: [] }, decide: true });
  assert.deepEqual(decideRequest("i", "release", { run: true }).body, { decision: "ready", run: true });
  assert.deepEqual(decideRequest("i", "needs_info", { questions: "one?\n two? " }).body, { decision: "needs_info", questions: ["one?", "two?"] });
  assert.deepEqual(decideRequest("i", "wontfix", { reason: "duplicate", evidence: " e ", duplicate_of: " t-1 " }).body,
    { decision: "wontfix", reason: "duplicate", evidence: "e", duplicate_of: "t-1" });
  assert.equal(decideRequest("i", "invalid"), null);
});

test("wontfix is refused before the round trip without a reason, evidence, or what it duplicates", () => {
  assert.match(decideProblem("wontfix", {}), /reason/);
  assert.match(decideProblem("wontfix", { reason: "invalid", evidence: " " }), /evidence/);
  assert.match(decideProblem("wontfix", { reason: "duplicate", evidence: "e" }), /duplicates/);
  assert.equal(decideProblem("wontfix", { reason: "out_of_scope", evidence: "e" }), null);
  assert.equal(decideProblem("release", {}), null);
});

test("only events touching an item or a triage run refresh the board", () => {
  assert.ok(touchesIntake({ type: "task_updated", task: { intake: { stage: "ready" } } }));
  assert.ok(touchesIntake({ type: "task_created", task: { labels: { "intake-triage": "x" } } }));
  assert.ok(touchesIntake({ type: "task_deleted", id: "x" }));
  assert.ok(!touchesIntake({ type: "task_updated", task: { labels: {} } }));
  assert.ok(!touchesIntake({ type: "run_updated", run: {} }));
  assert.ok(!touchesIntake(null));
});

test("ages read at a glance", () => {
  assert.equal(fmtAge(0), "1m");
  assert.equal(fmtAge(59 * 60), "59m");
  assert.equal(fmtAge(5 * 3600), "5h");
  assert.equal(fmtAge(3 * 86400 + 5), "3d");
});

test("a red item says what moves it, with the dialog for each action", () => {
  const next = nextActions(card("needs_info"));
  assert.deepEqual(next.map(n => [n.action, n.act]), [["split", "split"], ["add_info", "info"]]);
  assert.equal(next[0].reasons.length, 2);
  assert.match(next[0].hint, /triage it again for a proposal/);
  assert.deepEqual(nextActions(card("received")), []);
  assert.deepEqual(nextActions({ next_actions: [{ action: "unknown", reasons: [], hint: "" }] }), [], "only actions it can carry out");
});

test("a split opens on the proposal and is checked the daemon's way", () => {
  assert.deepEqual(splitDraft(card("needs_info")).map(p => p.title), ["", ""], "two parts to write without a proposal");
  const proposed = { triage: { assessment: { split: [
    { id: "resume", title: "Resume", instructions: "r" },
    { id: "rework", title: "Rework", depends_on: ["resume"], acceptance: "cargo test" },
  ] } } };
  const rows = splitDraft(proposed);
  assert.equal(rows[1].depends_on, "resume");
  const parts = buildParts(rows);
  assert.deepEqual(parts[1], { id: "rework", title: "Rework", instructions: "", depends_on: ["resume"], acceptance: "cargo test" });
  assert.equal(parts[0].depends_on, undefined);
  assert.equal(splitProblem(parts), null);

  assert.match(splitProblem(parts.slice(0, 1)), /two parts/);
  assert.match(splitProblem([parts[0], { ...parts[1], id: "resume" }]), /used twice/);
  assert.match(splitProblem([parts[0], { ...parts[1], id: "Re work" }]), /slug/);
  assert.match(splitProblem([parts[0], { ...parts[1], title: "" }]), /title/);
  assert.match(splitProblem([parts[0], { ...parts[1], depends_on: ["nope"] }]), /not a part/);
  assert.match(splitProblem([{ ...parts[0], depends_on: ["rework"] }, parts[1]]), /circle/);
  assert.match(splitProblem(Array.from({ length: 9 }, (_, i) => ({ id: `p${i}`, title: "t" }))), /at most 8/);

  assert.deepEqual(decideRequest("i", "split", { parts }).body, { decision: "split", parts });
});

test("a workflow route carries its inputs and each step's agent, checked against the board", () => {
  const values = {
    axes: board.axes.map(({ axis }) => ({ axis, pass: true, evidence: "ok" })),
    category: "feature", impact: "high", urgency: "medium", complexity: "5",
    scope: "demo", agent: "codex", workflow: "wf-1",
    inputs: { issue: " 178 " }, agents: { review: "codex", implement: "" },
    summary: "", questions: "",
  };
  const a = buildAssessment(values);
  assert.deepEqual(a.routing, { scope: "demo", workflow: "wf-1", inputs: { issue: "178" }, agents: { review: "codex" } },
    "an agent for the whole goes only without a workflow, and blank steps keep theirs");
  assert.equal(routeProblem(a, board), null);
  assert.match(routeProblem(buildAssessment({ ...values, inputs: {} }), board), /needs issue/);
  assert.match(routeProblem(buildAssessment({ ...values, agents: { deploy: "codex" } }), board), /no step deploy/);
  assert.match(routeProblem(buildAssessment({ ...values, workflow: "gone" }), board), /no workflow gone/);
  assert.equal(routeProblem(buildAssessment({ ...values, workflow: "" }), board), null);
  assert.deepEqual(buildAssessment({ ...values, workflow: "" }).routing, { scope: "demo", agent: "codex" });

  const proposal = buildAssessment({ ...values, split: [{ id: "a", title: "A" }] });
  assert.match(assessmentProblem(proposal), /proposed split: a split needs at least two parts/);

  const route = routeFor(board, "demo");
  assert.deepEqual(route.agents.map(agentText), ["claude-code (default model)", "builder (claude-code, opus)", "codex (gpt-5)"]);
  assert.deepEqual(routeFor(board, "elsewhere").workflows, []);
});

test("information can bring a triage run straight after it", () => {
  const req = infoRequest("i", "the answer", true);
  assert.deepEqual(req.body, { text: "the answer" });
  assert.deepEqual(req.after, triageRequest("i", ""));
  assert.equal(infoRequest("i", "x").after, undefined);
});

test("a part says which item it was split from", () => {
  assert.equal(cardNote({ stage: "received", parent: "40dd199e-6953" }), "part of item 40dd199e");
});
