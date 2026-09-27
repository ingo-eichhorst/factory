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
  basisText,
  buildAssessment,
  buildDuplicateAnswer,
  buildParts,
  canPublish,
  cardActions,
  cardNote,
  cards,
  confirmedDuplicate,
  decideProblem,
  decideRequest,
  duplicateRows,
  estimateOf,
  estimateText,
  fmtAge,
  flagSecurityRequest,
  infoRequest,
  nextActions,
  outboundInfo,
  previewVerdict,
  priorityOf,
  publishRequest,
  routeFor,
  routeProblem,
  securityDecisionProblem,
  securityDecisionRequest,
  securityFlag,
  splitDraft,
  splitProblem,
  totalOpen,
  touchesIntake,
  triageRequest,
  verdictChips,
  wontfixDraft,
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

test("basisText mirrors the daemon's EstimateBasis::describe exactly (#168)", () => {
  assert.equal(basisText(null), null, "no basis at all -- a Triage from before this existed");
  assert.equal(
    basisText({ source: "reference_class", scope: "factory", category: "bugfix", time_samples: 12 }),
    "p10–p90 of 12 completed bugfix tasks in factory, last 90 days",
  );
  assert.equal(
    basisText({ source: "reference_class", scope: "factory", category: "bugfix", time_samples: 1 }),
    "p10–p90 of 1 completed bugfix task in factory, last 90 days",
    "singular task, not tasks",
  );
  assert.equal(
    basisText({ source: "complexity_table", scope: "factory", category: "bugfix", time_samples: 2 }),
    "complexity table: 2 of 5 samples",
  );
  assert.equal(basisText({ source: "assessor" }), "the assessor's own estimate");
  // The fixture card predates #168: its triage carries no estimate_basis at
  // all, and verdictChips must read that as no basis, not throw.
  assert.equal(verdictChips(card("ready")).basis, null);
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
  assert.deepEqual(cardActions(card("needs_info")), ["info", "triage", "assess", "split", "wontfix", "flag_security"]);
  assert.deepEqual(cardActions({ ...card("ready"), stage: "split" }), [], "a split item is done with");
  assert.ok(cardActions(card("triaging")).includes("release"), "assessed ready waits for release");
  const running = { stage: "triaging", triage: null, triage_task: "t", triage_task_status: "running" };
  assert.ok(!cardActions(running).includes("triage"), "one triage run at a time");
  assert.ok(!cardActions({ ...running, triage_task_status: "failed" }).includes("release"));
  assert.ok(cardActions({ ...running, triage_task_status: "failed" }).includes("triage"));
});

test("the security fast lane gates release, split and wontfix, and only the owner-facing actions show while possible (#170)", () => {
  const base = card("needs_info");
  assert.equal(securityFlag(base), null, "the fixture carries no flag");
  assert.equal(securityFlag(null), null);

  const possible = { ...base, security: { state: "possible", flagged_by: "agent triager", flagged_at: "t", reason: "looks bad" } };
  const possibleActions = cardActions(possible);
  assert.ok(!possibleActions.includes("release"), "never released while possible");
  assert.ok(!possibleActions.includes("split"), "never split while possible");
  assert.ok(!possibleActions.includes("wontfix"), "never closed while possible");
  assert.ok(!possibleActions.includes("flag_security"), "already flagged");
  assert.ok(possibleActions.includes("security_confirm") && possibleActions.includes("security_dismiss"));
  assert.ok(possibleActions.includes("assess"), "assessing itself is still fine");

  const confirmed = { ...card("triaging"), security: { state: "confirmed", flagged_by: "x", flagged_at: "t", reason: "r" } };
  const confirmedActions = cardActions(confirmed);
  assert.ok(confirmedActions.includes("release"), "a confirmed report may still release");
  assert.ok(!confirmedActions.includes("wontfix"), "wontfix is for a dismissal, not a confirmed report");
  assert.ok(!confirmedActions.includes("flag_security"));
  assert.ok(!confirmedActions.includes("security_confirm") && !confirmedActions.includes("security_dismiss"), "already decided");

  const dismissed = { ...base, security: { state: "dismissed", flagged_by: "x", flagged_at: "t", reason: "r" } };
  const dismissedActions = cardActions(dismissed);
  assert.ok(dismissedActions.includes("wontfix"), "an ordinary item again");
  assert.ok(!dismissedActions.includes("flag_security"), "already carries a flag");
  assert.ok(!dismissedActions.includes("security_confirm") && !dismissedActions.includes("security_dismiss"));
});

test("a decided GitHub item with an outbound record offers publish, on a ready card too (#171)", () => {
  const github = (stage, outbound) => ({ ...card(stage), source: { kind: "github", reference: "https://github.com/acme/widgets/issues/9" }, outbound });
  const awaiting = { state: "awaiting_approval", by: "the owner", at: "t" };

  assert.ok(canPublish(github("needs_info", awaiting)));
  assert.ok(!canPublish(card("needs_info")), "a cli/ui item never publishes");
  assert.ok(!canPublish({ ...card("needs_info"), source: { kind: "github" } }), "no outbound recorded yet");

  const readyGithub = github("ready", awaiting);
  assert.deepEqual(cardActions(readyGithub), ["publish"], "a ready card otherwise has no actions (#119)");
  assert.ok(cardActions(github("needs_info", awaiting)).includes("publish"), "alongside the ordinary needs-info actions");

  assert.deepEqual(cardActions(github("ready", null)), [], "no outbound yet -- nothing decided for GitHub to publish");
});

test("outboundInfo shapes the awaiting/published/failed states for the modal (#171)", () => {
  assert.equal(outboundInfo(card("needs_info")), null, "the fixture carries nothing outbound");
  assert.equal(outboundInfo(null), null);

  const awaiting = { ...card("needs_info"), outbound: { state: "awaiting_approval", by: "the owner", at: "t" } };
  const info = outboundInfo(awaiting);
  assert.equal(info.state, "awaiting_approval");
  assert.equal(info.commentUrl, null);
  assert.deepEqual(info.labelsApplied, []);
  assert.deepEqual(info.labelsSkipped, []);
  assert.equal(info.error, null);

  const published = {
    ...card("needs_info"),
    outbound: {
      state: "published",
      comment_id: 501,
      comment_url: "https://github.com/acme/widgets/issues/9#issuecomment-501",
      labels_applied: ["needs-info"],
      labels_skipped: ["triage"],
      by: "the owner",
      at: "t",
    },
  };
  const publishedInfo = outboundInfo(published);
  assert.equal(publishedInfo.commentUrl, "https://github.com/acme/widgets/issues/9#issuecomment-501");
  assert.deepEqual(publishedInfo.labelsApplied, ["needs-info"]);
  assert.deepEqual(publishedInfo.labelsSkipped, ["triage"]);

  const failed = { ...card("needs_info"), outbound: { state: "failed", last_error: "gh: not found", by: "the owner", at: "t" } };
  assert.equal(outboundInfo(failed).error, "gh: not found");
});

test("publishRequest posts to the item's own publish route with no body fields", () => {
  assert.deepEqual(publishRequest("abc"), { path: "/api/intake/abc/publish", method: "POST", body: {} });
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

// -- definitions of ready (#169) --------------------------------------------

const definition = {
  scope: "demo",
  checks: [
    { id: "threat-model", pass_condition: "names a threat model", categories: ["security-report"], declared_at: { scope: "demo", file: "security" } },
    { id: "changelog", pass_condition: "the changelog is updated", categories: [], declared_at: { scope: "demo", file: "ready" } },
  ],
  max_complexity: 6,
  observability_tolerance: "medium",
  unreadable: [],
};

/// `buildAssessment` base values for the fixture's routes -- one axis
/// failing, per the shared `values` above, but every check answered.
function readyValues(overrides = {}) {
  return {
    axes: board.axes.map(({ axis }) => ({ axis, pass: true, evidence: `${axis} ok`, cost: null })),
    category: "security-report", impact: "high", urgency: "medium", complexity: "4",
    scope: "demo", agent: "", workflow: "", summary: "s", questions: "",
    checks: [
      { id: "threat-model", categories: ["security-report"], pass: true, evidence: "documented" },
      { id: "changelog", categories: [], pass: true, evidence: "updated" },
    ],
    ...overrides,
  };
}

test("buildAssessment drops a check that does not apply to the chosen category", () => {
  const a = buildAssessment(readyValues({ category: "bugfix" }));
  assert.deepEqual(a.checks, [{ id: "changelog", pass: true, evidence: "updated" }], "threat-model needs security-report");

  const b = buildAssessment(readyValues());
  assert.deepEqual(b.checks, [
    { id: "threat-model", pass: true, evidence: "documented" },
    { id: "changelog", pass: true, evidence: "updated" },
  ]);

  const none = buildAssessment({ ...readyValues(), checks: [] });
  assert.equal(none.checks, undefined, "no checks at all is left out entirely, like split and duplicates");
});

test("assessmentProblem requires evidence for every applicable check, and ignores the definition when there is none", () => {
  const a = buildAssessment(readyValues());
  assert.equal(assessmentProblem(a, [], definition), null);

  const missing = buildAssessment({ ...readyValues(), checks: [{ id: "changelog", categories: [], pass: true, evidence: "updated" }] });
  assert.match(assessmentProblem(missing, [], definition), /threat-model/);

  const noEvidence = buildAssessment(readyValues({
    checks: [
      { id: "threat-model", categories: ["security-report"], pass: true, evidence: " " },
      { id: "changelog", categories: [], pass: true, evidence: "updated" },
    ],
  }));
  assert.match(assessmentProblem(noEvidence, [], definition), /threat-model/);

  // Without a definition (a scope whose chain adds nothing), the same
  // assessment is fine -- nothing here is enforced.
  assert.equal(assessmentProblem(missing, []), null);
});

test("previewVerdict blocks on a failed applicable check and a tightened complexity cap, and honours the scope's own observability tolerance", () => {
  const a = buildAssessment(readyValues());
  assert.deepEqual(previewVerdict(a, definition), { verdict: "ready", blockers: [] });

  const failedCheck = buildAssessment(readyValues({
    checks: [
      { id: "threat-model", categories: ["security-report"], pass: false, evidence: "not written yet" },
      { id: "changelog", categories: [], pass: true, evidence: "updated" },
    ],
  }));
  assert.deepEqual(previewVerdict(failedCheck, definition).blockers, ["threat-model"]);

  const overCap = buildAssessment(readyValues({ complexity: "7" }));
  assert.deepEqual(previewVerdict(overCap, definition).blockers, ["complexity 7"]);
  assert.deepEqual(previewVerdict(overCap).blockers, [], "without a definition, 7 is under the default cap of 8");

  const nine = buildAssessment(readyValues({ complexity: "9" }));
  assert.deepEqual(previewVerdict(nine, definition).blockers, ["complexity 9"], "the fixed rule alone fires, not also the tighter cap");

  const failedObs = buildAssessment(readyValues({
    axes: board.axes.map(({ axis }) => ({ axis, pass: axis !== "observability", evidence: `${axis} ok`, cost: axis === "observability" ? "medium" : null })),
  }));
  assert.deepEqual(previewVerdict(failedObs, { ...definition, observability_tolerance: "low" }).blockers, ["observability"]);
  assert.deepEqual(previewVerdict(failedObs, { ...definition, observability_tolerance: "medium" }).blockers, []);
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

test("addRequest carries --security only when asked, and the security routes are the daemon's (#170)", () => {
  assert.equal(addRequest({ title: "x" }).body.security, undefined, "omitted when not asked");
  assert.equal(addRequest({ title: "x", security: true }).body.security, true);
  assert.equal(addRequest({ title: "x", security: false }).body.security, undefined);

  assert.deepEqual(flagSecurityRequest("a/b", " looks bad "), {
    path: "/api/intake/a%2Fb/flag-security", method: "POST", body: { reason: " looks bad " },
  });
  assert.deepEqual(flagSecurityRequest("i", undefined), { path: "/api/intake/i/flag-security", method: "POST", body: { reason: "" } });

  assert.deepEqual(securityDecisionRequest("i", "confirm", ""), {
    path: "/api/intake/i/security", method: "POST", body: { verdict: "confirm", evidence: "" },
  });
  assert.deepEqual(securityDecisionRequest("i", "dismiss", " false positive "), {
    path: "/api/intake/i/security", method: "POST", body: { verdict: "dismiss", evidence: "false positive" },
  });

  assert.equal(securityDecisionProblem("confirm", ""), null, "confirming needs no evidence");
  assert.equal(securityDecisionProblem("dismiss", "  "), "dismissing needs the evidence that clears it");
  assert.equal(securityDecisionProblem("dismiss", "false positive"), null);
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

// ------------------------------------------------------------ duplicates (#166)

const stored = { kind: "task", reference: "t-1", title: "Same bug", evidence: "same github reference", match: "source", verdict: "unverified" };

test("previewVerdict mirrors the duplicate blocker, ahead of the axes", () => {
  const values = {
    axes: board.axes.map(({ axis }) => ({ axis, pass: true, evidence: "ok" })),
    category: "bugfix", impact: "high", urgency: "medium", complexity: 4,
    scope: "demo", summary: "", questions: "",
    duplicates: [{ ...stored, verdict: "confirmed", evidence: "yes, same bug" }],
  };
  const a = buildAssessment(values);
  assert.deepEqual(previewVerdict(a), { verdict: "needs_info", blockers: ["duplicate t-1"] });

  const rejected = buildAssessment({ ...values, duplicates: [{ ...stored, verdict: "rejected", evidence: "no, different cause" }] });
  assert.deepEqual(previewVerdict(rejected), { verdict: "ready", blockers: [] });
});

test("assessmentProblem refuses a stored candidate left unanswered or answered without evidence", () => {
  const values = {
    axes: board.axes.map(({ axis }) => ({ axis, pass: true, evidence: "ok" })),
    category: "bugfix", impact: "high", urgency: "medium", complexity: 4,
    scope: "demo", summary: "", questions: "",
  };
  const unanswered = buildAssessment(values);
  assert.match(assessmentProblem(unanswered, [stored]), /confirm or reject it/);
  assert.equal(assessmentProblem(unanswered, []), null, "nothing stored, nothing to answer");

  const noEvidence = buildAssessment({ ...values, duplicates: [{ ...stored, verdict: "confirmed", evidence: " " }] });
  assert.match(assessmentProblem(noEvidence, [stored]), /evidence for its verdict/);

  const answered = buildAssessment({ ...values, duplicates: [{ ...stored, verdict: "rejected", evidence: "not the same" }] });
  assert.equal(assessmentProblem(answered, [stored]), null);
});

test("buildDuplicateAnswer shapes a row to the wire, dropping a blank knowledge row", () => {
  assert.deepEqual(buildDuplicateAnswer({ ...stored, verdict: "confirmed", evidence: " yes " }), {
    kind: "task", reference: "t-1", title: "Same bug", match: "source", verdict: "confirmed", evidence: "yes",
  });
  const knowledge = buildDuplicateAnswer({
    kind: "knowledge", reference: "specs/x.md", title: "X", match: "text", score: "80", verdict: "confirmed", evidence: "documents it",
  });
  assert.equal(knowledge.score, 80, "a text match's score is a number");
  assert.deepEqual(buildAssessment({
    axes: board.axes.map(({ axis }) => ({ axis, pass: true, evidence: "ok" })),
    category: "bugfix", impact: "high", urgency: "medium", complexity: 4, scope: "demo", summary: "", questions: "",
    duplicates: [{ kind: "", reference: "" }],
  }).duplicates, undefined, "a row naming neither kind nor reference is dropped");
});

test("duplicateRows normalises a card's candidates for the modal", () => {
  const card = { candidates: [
    { kind: "task", reference: "t-1", title: "Same bug", evidence: "same reference", match: "source", verdict: "unverified" },
    { kind: "knowledge", reference: "specs/x.md", title: "X", evidence: "documents it", match: "text", score: 82, verdict: "confirmed" },
  ] };
  const rows = duplicateRows(card);
  assert.equal(rows[0].matchText, "source");
  assert.equal(rows[1].matchText, "text 82%");
  assert.deepEqual(duplicateRows({}), []);
});

test("a wontfix dialog prefills from the assessment's own confirmed duplicate", () => {
  const confirmedCard = { triage: { assessment: { duplicates: [
    { ...stored, verdict: "rejected", evidence: "not this one" },
    { ...stored, reference: "t-2", verdict: "confirmed", evidence: "same bug, twice" },
  ] } } };
  assert.deepEqual(confirmedDuplicate(confirmedCard), { ...stored, reference: "t-2", verdict: "confirmed", evidence: "same bug, twice" });
  assert.deepEqual(wontfixDraft(confirmedCard), { reason: "duplicate", duplicate_of: "t-2", evidence: "same bug, twice" });

  const noneConfirmed = { triage: { assessment: { duplicates: [{ ...stored, verdict: "rejected", evidence: "not this one" }] } } };
  assert.equal(confirmedDuplicate(noneConfirmed), null);
  assert.deepEqual(wontfixDraft(noneConfirmed), { reason: "", duplicate_of: "", evidence: "" });
  assert.deepEqual(wontfixDraft({}), { reason: "", duplicate_of: "", evidence: "" });
});

test("a confirmed candidate's close_duplicate next action opens the wontfix dialog", () => {
  const card = { next_actions: [
    { action: "close_duplicate", reasons: ["Duplicate: confirmed duplicate of t-2 -- same bug, twice"], hint: "Close it as the duplicate it was confirmed to be -- a person still decides, never the triage run.", reference: "t-2" },
  ] };
  assert.deepEqual(nextActions(card), [
    { label: "Close as duplicate", act: "wontfix", action: "close_duplicate", hint: card.next_actions[0].hint, reasons: card.next_actions[0].reasons },
  ]);
});
