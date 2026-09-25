//! Pure shaping for the L4 Intake view (`#119`): what a card says, which
//! actions an item allows, and the requests those actions send. No DOM and
//! no fetch -- `intake.js` draws and wires, the split `operations-model.js`
//! keeps. Every rule that decides anything (ready or needs-info, the
//! priority, the estimate) is the daemon's (`factory_core::intake`); the
//! copies here only preview it in the assessment form, and the daemon's
//! answer is what the board shows.

/// The four columns, in the order work moves through them.
export const COLUMNS = [
  { key: "received", label: "Received", hint: "handed in, nobody has looked yet" },
  { key: "triaging", label: "Triaging", hint: "a triage run or an assessment waiting for a decision" },
  { key: "needs_info", label: "Needs info", hint: "sent back to the requester" },
  { key: "ready", label: "Ready", hint: "released into the line, last 14 days" },
];

export const LEVELS = ["high", "medium", "low"];
export const COSTS = ["low", "medium", "high"];
export const WONTFIX_REASONS = [
  { key: "duplicate", label: "Duplicate" },
  { key: "invalid", label: "Invalid" },
  { key: "out_of_scope", label: "Out of scope" },
];

/// Impact x urgency, the daemon's matrix (`factory_core::intake::priority`)
/// -- only for the form's live preview.
export function priorityOf(impact, urgency) {
  const rank = { high: 0, medium: 1, low: 2 };
  const sum = rank[impact] + rank[urgency];
  if (Number.isNaN(sum)) return null;
  return ["P1", "P2", "P3"][sum] || "P4";
}

/// `ir:triage`'s complexity table in seconds, or null for 9-10.
export function estimateOf(complexity) {
  const c = Number(complexity);
  const M = 60, H = 3600;
  if (c === 1 || c === 2) return { min_seconds: 15 * M, max_seconds: 45 * M };
  if (c === 3 || c === 4) return { min_seconds: 45 * M, max_seconds: 2 * H };
  if (c === 5 || c === 6) return { min_seconds: 90 * M, max_seconds: 3 * H };
  if (c === 7 || c === 8) return { min_seconds: 150 * M, max_seconds: 5 * H };
  return null;
}

function shortDuration(seconds) {
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  if (minutes % 60 === 0) return `${minutes / 60}h`;
  return `${Math.floor(minutes / 60)}h${minutes % 60}m`;
}

/// `45m-2h`, the daemon's own spelling (`Estimate::describe`).
export function estimateText(estimate) {
  if (!estimate) return "no estimate";
  return `${shortDuration(estimate.min_seconds)}-${shortDuration(estimate.max_seconds)}`;
}

/// How long an item has waited, at a glance.
export function fmtAge(seconds) {
  const s = Math.max(0, Math.floor(seconds || 0));
  if (s < 3600) return `${Math.max(1, Math.floor(s / 60))}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}

export function cards(board, key) {
  return (board && board.columns && board.columns[key]) || [];
}

export function totalOpen(board) {
  return ["received", "triaging", "needs_info"].reduce((n, k) => n + cards(board, k).length, 0);
}

/// The seven axes of a card, in the board's order, each `pass`, `fail`,
/// `tolerated` (a failed observability at low or medium cost, which does not
/// block) or `unassessed`.
export function axisMarks(card, axes) {
  const checks = (card && card.triage && card.triage.assessment && card.triage.assessment.axes) || [];
  return (axes || []).map(({ axis, label, pass_condition }) => {
    const check = checks.find(c => c.axis === axis);
    let mark = "unassessed";
    if (check) {
      if (check.pass) mark = "pass";
      else if (axis === "observability" && (check.cost === "low" || check.cost === "medium")) mark = "tolerated";
      else mark = "fail";
    }
    return { axis, label, pass_condition, mark, evidence: check ? check.evidence : "", cost: check ? check.cost || null : null };
  });
}

/// Category, priority and estimate, once an item is assessed.
export function verdictChips(card) {
  const t = card && card.triage;
  if (!t) return null;
  return {
    priority: t.priority,
    category: t.assessment.category,
    estimate: estimateText(t.estimate),
    verdict: t.verdict.verdict,
  };
}

/// The card's triage task has ended and nothing is running it. The daemon
/// says so in `triage_task_ended` since #122 -- a failed triage run leaves
/// its task `blocked`, which the status alone cannot tell from one waiting
/// on a question; an older daemon without the field is read off the status.
export function triageEnded(card) {
  if (!card) return false;
  if (typeof card.triage_task_ended === "boolean") return card.triage_task_ended;
  return ["done", "failed", "cancelled"].includes(card.triage_task_status);
}

/// One line under a card saying what is happening to it, or null.
export function cardNote(card) {
  if (!card) return null;
  const run = card.triage_task_status;
  if (card.stage === "triaging" && !card.triage && card.triage_task) {
    if (triageEnded(card)) {
      const how = run === "blocked" ? "failed" : run;
      return `the triage run ended (${how}) without an assessment -- triage it again or assess it by hand`;
    }
    return `triage run ${run || "starting"}`;
  }
  if (card.stage === "triaging" && card.triage) {
    return card.triage.verdict.verdict === "ready"
      ? "assessed ready -- waiting for release"
      : "assessed needs-info -- waiting for a decision";
  }
  if (card.stage === "ready" && card.decision) {
    return card.decision.workflow_run
      ? `released into workflow run ${card.decision.workflow_run.slice(0, 8)}`
      : `released as ${card.status}`;
  }
  return null;
}

/// Which actions an item allows, in the order the buttons are drawn. A
/// released item has none: it is a task now, and the task modal is where it
/// is worked.
export function cardActions(card) {
  if (!card || card.stage === "ready" || card.stage === "wontfix") return [];
  const triageRunning = card.triage_task && !card.triage && !triageEnded(card);
  const out = [];
  if (card.stage === "needs_info") out.push("info");
  if (!triageRunning) out.push("triage");
  out.push("assess");
  if (card.triage && card.triage.verdict.verdict === "ready") out.push("release");
  if (card.stage !== "needs_info") out.push("needs_info");
  out.push("wontfix");
  return out;
}

export const ACTION_LABELS = {
  info: "Add information",
  triage: "Triage with an agent",
  assess: "Assess",
  release: "Release",
  needs_info: "Needs info",
  wontfix: "Won't fix",
};

// ------------------------------------------------------------ requests

export function addRequest(values) {
  const body = { title: (values.title || "").trim(), instructions: values.instructions || "", source: "ui" };
  if (values.scope) body.scope = values.scope;
  if ((values.reference || "").trim()) body.reference = values.reference.trim();
  if ((values.requester || "").trim()) body.requester = values.requester.trim();
  return { path: "/api/intake", method: "POST", body };
}

export function triageRequest(id, agent) {
  const body = {};
  if ((agent || "").trim()) body.agent = agent.trim();
  return { path: `/api/intake/${encodeURIComponent(id)}/triage`, method: "POST", body };
}

export function infoRequest(id, text) {
  return { path: `/api/intake/${encodeURIComponent(id)}/info`, method: "POST", body: { text } };
}

const lines = (text) => String(text || "").split("\n").map(s => s.trim()).filter(Boolean);

/// The decision body for release, needs-info or wontfix -- `Decision` on
/// the wire, tagged by `decision`.
export function decideRequest(id, action, values = {}) {
  let body;
  if (action === "release") body = { decision: "ready", run: !!values.run };
  else if (action === "needs_info") body = { decision: "needs_info", questions: lines(values.questions) };
  else if (action === "wontfix") {
    body = { decision: "wontfix", reason: values.reason, evidence: (values.evidence || "").trim() };
    if (values.reason === "duplicate") body.duplicate_of = (values.duplicate_of || "").trim();
  } else return null;
  return { path: `/api/intake/${encodeURIComponent(id)}/decide`, method: "POST", body };
}

/// What stops a decision being sent, as a sentence, or null -- the
/// daemon's own rules, said before the round trip.
export function decideProblem(action, values = {}) {
  if (action === "wontfix") {
    if (!values.reason) return "choose a reason";
    if (!(values.evidence || "").trim()) return "wontfix needs the evidence that verifies it";
    if (values.reason === "duplicate" && !(values.duplicate_of || "").trim()) return "name what it duplicates";
  }
  return null;
}

/// The assessment form's values as the wire's `Assessment`.
export function buildAssessment(values) {
  const axes = (values.axes || []).map(a => {
    const check = { axis: a.axis, pass: !!a.pass, evidence: (a.evidence || "").trim() };
    if (a.axis === "observability" && !a.pass && a.cost) check.cost = a.cost;
    return check;
  });
  const routing = { scope: values.scope || "" };
  if ((values.agent || "").trim()) routing.agent = values.agent.trim();
  if ((values.workflow || "").trim()) routing.workflow = values.workflow.trim();
  return {
    axes,
    category: (values.category || "").trim(),
    impact: values.impact,
    urgency: values.urgency,
    complexity: Number(values.complexity),
    routing,
    summary: (values.summary || "").trim(),
    questions: lines(values.questions),
  };
}

/// Mirror of `factory_core::intake::validate`, for the form: the first
/// thing that would be refused, or null.
export function assessmentProblem(a) {
  for (const check of a.axes) {
    if (!check.evidence) return `${check.axis}: one sentence of evidence`;
    if (check.axis === "observability" && !check.pass && !check.cost) return "a failed observability axis needs its cost";
  }
  if (a.axes.length !== 7) return "all seven axes are needed";
  if (!/^[a-z0-9-]+$/.test(a.category)) return "category: a slug, as in bugfix";
  if (!(a.complexity >= 1 && a.complexity <= 10)) return "complexity: 1 to 10";
  if (!LEVELS.includes(a.impact) || !LEVELS.includes(a.urgency)) return "impact and urgency: high, medium or low";
  if (!a.routing.scope) return "route it to a scope";
  return null;
}

/// What the rules will make of the form as it stands: `ready`, or
/// `needs_info` with the reasons.
export function previewVerdict(a) {
  const blockers = [];
  for (const check of a.axes) {
    if (check.pass) continue;
    if (check.axis === "observability" && (check.cost === "low" || check.cost === "medium")) continue;
    blockers.push(check.axis);
  }
  if (a.complexity >= 9) blockers.push(`complexity ${a.complexity}`);
  return blockers.length ? { verdict: "needs_info", blockers } : { verdict: "ready", blockers };
}

export function assessRequest(id, assessment, decide) {
  return {
    path: `/api/intake/${encodeURIComponent(id)}/assess`,
    method: "POST",
    body: { assessment, decide: !!decide },
  };
}

/// Does this socket event change the board? Anything touching an intake
/// item or a triage run -- not every task's progress.
export function touchesIntake(ev) {
  if (!ev || !ev.type) return false;
  if (ev.type === "task_deleted") return true;
  const t = ev.task;
  if ((ev.type === "task_created" || ev.type === "task_updated") && t) {
    return !!t.intake || !!(t.labels && t.labels["intake-triage"]);
  }
  return false;
}
