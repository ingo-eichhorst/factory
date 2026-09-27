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

/// What an estimate rests on (`#168`, `EstimateBasis::describe` mirrored
/// exactly): "p10-p90 of 12 completed bugfix tasks in factory, last 90
/// days" for a reference class, "complexity table: 2 of 5 samples" for the
/// fallback, "the assessor's own estimate" for one an assessor gave outright,
/// null only for a `Triage` from before this existed (no `estimate_basis` at
/// all) -- the one case with truly nothing to say.
export function basisText(basis) {
  if (!basis) return null;
  if (basis.source === "reference_class") {
    const n = basis.time_samples;
    return `p10–p90 of ${n} completed ${basis.category} task${n === 1 ? "" : "s"} in ${basis.scope}, last 90 days`;
  }
  if (basis.source === "complexity_table") {
    return `complexity table: ${basis.time_samples} of 5 samples`;
  }
  if (basis.source === "assessor") {
    return "the assessor's own estimate";
  }
  return null;
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
    basis: basisText(t.estimate_basis),
    verdict: t.verdict.verdict,
  };
}

// ------------------------------------------------------------ duplicates

/// `card.candidates` (the daemon's search overlaid with the triager's own
/// answer, `factory_core::intake::candidates_with_verdicts`), normalised
/// for the item modal: `kind`, `reference`, `title`, `evidence`, `verdict`
/// and `matchText` -- "source" or "text 82%".
export function duplicateRows(card) {
  return ((card && card.candidates) || []).map(c => ({
    kind: c.kind,
    reference: c.reference,
    title: c.title,
    evidence: c.evidence,
    verdict: c.verdict,
    matchText: c.match === "text" ? `text ${c.score != null ? c.score : "?"}%` : "source",
  }));
}

/// The first candidate the assessment itself confirmed -- what a wontfix
/// dialog prefills `duplicate_of` from. Null when there is none.
export function confirmedDuplicate(card) {
  const duplicates = (card && card.triage && card.triage.assessment && card.triage.assessment.duplicates) || [];
  return duplicates.find(d => d.verdict === "confirmed") || null;
}

/// The wontfix dialog's starting values: a confirmed duplicate's own
/// reference and evidence when there is one, otherwise blank.
export function wontfixDraft(card) {
  const d = confirmedDuplicate(card);
  return d ? { reason: "duplicate", duplicate_of: d.reference, evidence: d.evidence || "" } : { reason: "", duplicate_of: "", evidence: "" };
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

/// The note for a card whose triage run has not assessed it: still going,
/// or ended without an assessment.
function triageRunNote(card) {
  const run = card.triage_task_status;
  if (!triageEnded(card)) return `triage run ${run || "starting"}`;
  const how = run === "blocked" ? "failed" : run;
  return `the triage run ended (${how}) without an assessment -- triage it again or assess it by hand`;
}

/// One line under a card saying what is happening to it, or null.
export function cardNote(card) {
  if (!card) return null;
  if (card.stage === "triaging" && !card.triage && card.triage_task) return triageRunNote(card);
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
  if (card.parent) return `part of item ${card.parent.slice(0, 8)}`;
  return null;
}

/// A possible or confirmed security report (`#170`) -- the fast lane
/// `board()` already sorted; here it is what gates release, split and
/// wontfix. `dismissed` is an ordinary item again.
export function securityFlag(card) {
  return (card && card.security) || null;
}

/// A decided GitHub item with something to publish (`#171`): the daemon
/// only ever records `outbound` for a GitHub-sourced item once it carries
/// both a decision and the assessment behind it, and never for a possible
/// or confirmed security report -- so its mere presence is the whole
/// eligibility check; a dismissed report's item is ordinary again the same
/// way `securityFlag` treats it everywhere else.
export function canPublish(card) {
  return !!(card && card.source && card.source.kind === "github" && card.outbound);
}

/// Which actions an item allows, in the order the buttons are drawn. A
/// released item is a task now, and the task modal is where it is worked --
/// except approving a GitHub publish, which stays available there too,
/// since that is the one thing left to do with it here. Any open item can
/// be split -- a person may cut one nobody could assess as a whole --
/// except a possible security report, which nothing may release, split or
/// close until a person looks (`#170`).
export function cardActions(card) {
  if (!card) return [];
  if (card.stage === "ready") return canPublish(card) ? ["publish"] : [];
  if (!["received", "triaging", "needs_info"].includes(card.stage)) return [];
  const triageRunning = card.triage_task && !card.triage && !triageEnded(card);
  const flag = securityFlag(card);
  const possible = !!flag && flag.state === "possible";
  const confirmed = !!flag && flag.state === "confirmed";
  const out = [];
  if (card.stage === "needs_info") out.push("info");
  if (!triageRunning) out.push("triage");
  out.push("assess");
  if (!possible && card.triage && card.triage.verdict.verdict === "ready") out.push("release");
  if (!possible) out.push("split");
  if (card.stage !== "needs_info") out.push("needs_info");
  // Wontfix is for a dismissal, never for a report nobody has looked at yet
  // or one already confirmed real.
  if (!possible && !confirmed) out.push("wontfix");
  // Flagging only adds scrutiny, so it stays open to anyone who could assess
  // the item -- but only once: an item already carrying a flag of any kind
  // (possible, confirmed or dismissed) is never flagged a second time.
  if (!flag) out.push("flag_security");
  // Confirm or dismiss: the owner's alone (the daemon refuses anyone else),
  // and only while the report is still `possible`.
  if (possible) out.push("security_confirm", "security_dismiss");
  // Approve and post to GitHub: a needs-info item can carry an outbound
  // record too (a decision sent it back, and it came from GitHub).
  if (canPublish(card)) out.push("publish");
  return out;
}

export const ACTION_LABELS = {
  info: "Add information",
  triage: "Triage with an agent",
  assess: "Assess",
  release: "Release",
  split: "Split into items",
  needs_info: "Needs info",
  wontfix: "Won't fix",
  flag_security: "Flag as security report",
  security_confirm: "Confirm security report",
  security_dismiss: "Dismiss security report",
  publish: "Approve and post to GitHub",
};

/// The daemon's next actions (`factory_core::intake::next_actions`), each
/// with the dialog that carries it out. A confirmed duplicate opens the
/// same wontfix dialog the board's own button does -- `wontfixDraft` is
/// what prefills it.
const NEXT_ACTION = {
  close_duplicate: { label: "Close as duplicate", act: "wontfix" },
  split: { label: "Split into items", act: "split" },
  add_info: { label: "Add information", act: "info" },
};

/// What would move a held-back item, as the item dialog draws it: the
/// daemon's own list, with a button per action. Empty for any other item.
export function nextActions(card) {
  return ((card && card.next_actions) || [])
    .filter(n => NEXT_ACTION[n.action])
    .map(n => ({ ...NEXT_ACTION[n.action], action: n.action, hint: n.hint, reasons: n.reasons || [] }));
}

// ---------------------------------------------------------------- split

/// The parts a split dialog opens with: the assessment's proposal, or two
/// empty ones to write.
export function splitDraft(card) {
  const proposed = (card && card.triage && card.triage.assessment.split) || [];
  if (proposed.length) {
    return proposed.map(p => ({
      id: p.id, title: p.title, instructions: p.instructions || "",
      depends_on: (p.depends_on || []).join(", "), acceptance: p.acceptance || "",
    }));
  }
  return [emptyPart(1), emptyPart(2)];
}

export function emptyPart(n) {
  return { id: `part-${n}`, title: "", instructions: "", depends_on: "", acceptance: "" };
}

const commaList = (text) => String(text || "").split(",").map(s => s.trim()).filter(Boolean);

/// The form's parts as the wire's `SplitPart`s.
export function buildParts(rows) {
  return (rows || []).map(r => {
    const part = { id: (r.id || "").trim(), title: (r.title || "").trim(), instructions: (r.instructions || "").trim() };
    const deps = commaList(r.depends_on);
    if (deps.length) part.depends_on = deps;
    if ((r.acceptance || "").trim()) part.acceptance = r.acceptance.trim();
    return part;
  });
}

/// Mirror of `factory_core::intake::validate_split`: the first thing that
/// would be refused, or null.
export function splitProblem(parts) {
  if (parts.length < 2) return "a split needs at least two parts";
  if (parts.length > MAX_SPLIT_PARTS) return `at most ${MAX_SPLIT_PARTS} parts`;
  const ids = new Set();
  for (const p of parts) {
    if (!/^[a-z0-9-]+$/.test(p.id)) return `part id "${p.id}": a slug, as in resume-mechanism`;
    if (ids.has(p.id)) return `part id "${p.id}" is used twice`;
    ids.add(p.id);
    if (!p.title) return `part ${p.id} needs a title`;
  }
  for (const p of parts) {
    for (const d of p.depends_on || []) {
      if (d === p.id) return `part ${p.id} depends on itself`;
      if (!ids.has(d)) return `part ${p.id} depends on "${d}", which is not a part`;
    }
  }
  const placed = new Set();
  while (placed.size < parts.length) {
    const next = parts.find(p => !placed.has(p.id) && (p.depends_on || []).every(d => placed.has(d)));
    if (!next) return "the parts' dependencies go round in a circle";
    placed.add(next.id);
  }
  return null;
}

export const MAX_SPLIT_PARTS = 8;

// --------------------------------------------------------------- routes

/// The board's route for one scope: its agents (with models) and workflows.
export function routeFor(board, scope) {
  return ((board && board.routes) || []).find(r => r.scope === scope) || { scope, agents: [], workflows: [] };
}

/// A workflow of a route, by id or by name -- an assessment names either.
export function workflowIn(route, wanted) {
  if (!wanted) return null;
  return (route.workflows || []).find(w => w.id === wanted || w.name === wanted) || null;
}

/// `builder (claude-code, opus)`: an agent as a select option says it.
export function agentText(agent) {
  return `${agent.name} (${agent.harness === agent.name ? "" : `${agent.harness}, `}${agent.model || "default model"})`;
}

// ------------------------------------------------------------ requests

export function addRequest(values) {
  const body = { title: (values.title || "").trim(), instructions: values.instructions || "", source: "ui" };
  if (values.scope) body.scope = values.scope;
  if ((values.reference || "").trim()) body.reference = values.reference.trim();
  if ((values.requester || "").trim()) body.requester = values.requester.trim();
  if (values.security) body.security = true;
  return { path: "/api/intake", method: "POST", body };
}

/// `#170`: flag an item still in the gate as a possible security report.
export function flagSecurityRequest(id, reason) {
  return { path: `/api/intake/${encodeURIComponent(id)}/flag-security`, method: "POST", body: { reason: reason || "" } };
}

/// `#170`: a person confirms or dismisses. `verdict` is `"confirm"` or
/// `"dismiss"`; evidence is required for a dismissal, optional to confirm.
export function securityDecisionRequest(id, verdict, evidence) {
  return {
    path: `/api/intake/${encodeURIComponent(id)}/security`,
    method: "POST",
    body: { verdict, evidence: (evidence || "").trim() },
  };
}

/// What stops a security decision being sent -- the daemon's own rule said
/// before the round trip.
export function securityDecisionProblem(verdict, evidence) {
  if (verdict === "dismiss" && !(evidence || "").trim()) {
    return "dismissing needs the evidence that clears it";
  }
  return null;
}

export function triageRequest(id, agent) {
  const body = {};
  if ((agent || "").trim()) body.agent = agent.trim();
  return { path: `/api/intake/${encodeURIComponent(id)}/triage`, method: "POST", body };
}

/// Information, and -- when asked -- a triage run straight after it, as
/// the follow-up request `after`.
export function infoRequest(id, text, retriage = false) {
  const req = { path: `/api/intake/${encodeURIComponent(id)}/info`, method: "POST", body: { text } };
  if (retriage) req.after = triageRequest(id, "");
  return req;
}

const lines = (text) => String(text || "").split("\n").map(s => s.trim()).filter(Boolean);

/// The decision body for release, needs-info or wontfix -- `Decision` on
/// the wire, tagged by `decision`.
export function decideRequest(id, action, values = {}) {
  let body;
  if (action === "release") body = { decision: "ready", run: !!values.run };
  else if (action === "split") body = { decision: "split", parts: values.parts || [] };
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

/// One duplicate row -- a stored candidate answered, or a knowledge one
/// added by hand -- as the wire's `DuplicateCandidate`. Dropped by
/// `buildAssessment` when it names neither a kind nor a reference: an
/// "add a knowledge candidate" row nobody filled in.
export function buildDuplicateAnswer(row) {
  const out = {
    kind: row.kind,
    reference: (row.reference || "").trim(),
    title: (row.title || "").trim(),
    match: row.match || "text",
    verdict: row.verdict || "unverified",
    evidence: (row.evidence || "").trim(),
  };
  if (out.match === "text" && row.score !== "" && row.score != null && !Number.isNaN(Number(row.score))) {
    out.score = Number(row.score);
  }
  return out;
}

/// The assessment form's values as the wire's `Assessment`.
export function buildAssessment(values) {
  const axes = (values.axes || []).map(a => {
    const check = { axis: a.axis, pass: !!a.pass, evidence: (a.evidence || "").trim() };
    if (a.axis === "observability" && !a.pass && a.cost) check.cost = a.cost;
    return check;
  });
  const routing = { scope: values.scope || "" };
  const workflow = (values.workflow || "").trim();
  if (workflow) {
    routing.workflow = workflow;
    const inputs = clean(values.inputs);
    if (Object.keys(inputs).length) routing.inputs = inputs;
    const agents = clean(values.agents);
    if (Object.keys(agents).length) routing.agents = agents;
  } else if ((values.agent || "").trim()) {
    routing.agent = values.agent.trim();
  }
  const split = buildParts(values.split);
  const duplicates = (values.duplicates || []).map(buildDuplicateAnswer).filter(d => d.kind && d.reference);
  const category = (values.category || "").trim();
  // A scope's own extra checks (`#169`): each row carries the categories it
  // applies to (from the routed scope's effective definition), so a check
  // that does not apply to the category just chosen is left out here rather
  // than submitted and refused by the daemon's own `validate`.
  const checks = (values.checks || [])
    .filter(c => !(c.categories || []).length || c.categories.includes(category))
    .map(c => ({ id: c.id, pass: !!c.pass, evidence: (c.evidence || "").trim() }));
  return {
    axes,
    category,
    impact: values.impact,
    urgency: values.urgency,
    complexity: Number(values.complexity),
    routing,
    summary: (values.summary || "").trim(),
    questions: lines(values.questions),
    ...(split.length ? { split } : {}),
    ...(duplicates.length ? { duplicates } : {}),
    ...(checks.length ? { checks } : {}),
  };
}

/// An object's non-blank values, trimmed.
function clean(map) {
  const out = {};
  for (const [k, v] of Object.entries(map || {})) {
    if (String(v || "").trim()) out[k] = String(v).trim();
  }
  return out;
}

/// The checks of `definition` (`ReadyDefinition`, `#169`) that apply to
/// `category`: none declared for it (the default, "every category"), or
/// `category` named explicitly -- `ReadyDefinition::applicable` in JS.
function applicableChecks(definition, category) {
  return ((definition && definition.checks) || []).filter(
    c => !(c.categories || []).length || c.categories.includes(category)
  );
}

/// Mirror of `factory_core::intake::validate` and `validate_duplicates`,
/// for the form: the first thing that would be refused, or null. `stored`
/// is the item's own found candidates (`card.candidates`) -- omitted where
/// there are none to leave unanswered. `definition` is the routed scope's
/// effective definition of ready (`#169`, `route.definition`) -- omitted
/// for a scope whose chain adds nothing, where every existing check still
/// passes unchanged.
export function assessmentProblem(a, stored = [], definition = null) {
  for (const check of a.axes) {
    if (!check.evidence) return `${check.axis}: one sentence of evidence`;
    if (check.axis === "observability" && !check.pass && !check.cost) return "a failed observability axis needs its cost";
  }
  if (a.axes.length !== 7) return "all seven axes are needed";
  if (!/^[a-z0-9-]+$/.test(a.category)) return "category: a slug, as in bugfix";
  if (!(a.complexity >= 1 && a.complexity <= 10)) return "complexity: 1 to 10";
  if (!LEVELS.includes(a.impact) || !LEVELS.includes(a.urgency)) return "impact and urgency: high, medium or low";
  if (!a.routing.scope) return "route it to a scope";
  if (a.split && a.split.length) {
    const why = splitProblem(a.split);
    if (why) return `the proposed split: ${why}`;
  }
  for (const d of a.duplicates || []) {
    if (!(d.evidence || "").trim()) return `duplicate ${d.reference || "candidate"}: evidence for its verdict`;
  }
  for (const s of stored) {
    const answer = (a.duplicates || []).find(d => d.kind === s.kind && d.reference === s.reference);
    if (!answer || answer.verdict === "unverified") return `possible duplicate ${s.reference}: confirm or reject it`;
  }
  for (const c of applicableChecks(definition, a.category)) {
    const answer = (a.checks || []).find(x => x.id === c.id);
    if (!answer || !(answer.evidence || "").trim()) return `check ${c.id}: one sentence of evidence`;
  }
  return null;
}

/// The route checked against what the board says exists: a workflow the
/// scope has, every input it declares, and steps it has. Null when fine.
export function routeProblem(a, board) {
  if (!a.routing.workflow) return null;
  const w = workflowIn(routeFor(board, a.routing.scope), a.routing.workflow);
  if (!w) return `${a.routing.scope} has no workflow ${a.routing.workflow}`;
  const given = a.routing.inputs || {};
  const missing = (w.inputs || []).filter(i => !given[i.name]).map(i => i.name);
  if (missing.length) return `workflow ${w.name} needs ${missing.join(", ")}`;
  const steps = new Set((w.steps || []).map(s => s.id));
  const unknown = Object.keys(a.routing.agents || {}).find(s => !steps.has(s));
  if (unknown) return `workflow ${w.name} has no step ${unknown}`;
  return null;
}

/// Whether a failed observability axis at `cost` passes through `tolerance`
/// -- `factory_core::ready::Tolerance::allows`, mirrored: at `medium` (the
/// default, and today's only rule) low or medium cost passes, at `low` only
/// low does, and `none` tolerates nothing.
function toleranceAllows(tolerance, cost) {
  if (tolerance === "low") return cost === "low";
  if (tolerance === "none") return false;
  return cost === "low" || cost === "medium";
}

/// What the rules will make of the form as it stands: `ready`, or
/// `needs_info` with the reasons. Mirrors `evaluate`'s order -- a confirmed
/// duplicate first, then the axes, then the scope's own extra checks
/// (`#169`), then complexity. `definition` is the routed scope's effective
/// definition of ready, as in `assessmentProblem`; omitted, this previews
/// exactly what it always has.
export function previewVerdict(a, definition = null) {
  const blockers = [];
  for (const d of a.duplicates || []) {
    if (d.verdict === "confirmed") blockers.push(`duplicate ${d.reference}`);
  }
  const tolerance = (definition && definition.observability_tolerance) || "medium";
  for (const check of a.axes) {
    if (check.pass) continue;
    if (check.axis === "observability" && toleranceAllows(tolerance, check.cost)) continue;
    blockers.push(check.axis);
  }
  for (const c of applicableChecks(definition, a.category)) {
    const answer = (a.checks || []).find(x => x.id === c.id);
    if (answer && !answer.pass) blockers.push(c.id);
  }
  if (a.complexity >= 9) blockers.push(`complexity ${a.complexity}`);
  else if (definition && a.complexity > (definition.max_complexity || 8)) blockers.push(`complexity ${a.complexity}`);
  return blockers.length ? { verdict: "needs_info", blockers } : { verdict: "ready", blockers };
}

/// `#171`: approve and post a decided GitHub item's triage comment and
/// labels to the issue it came from. Factory never does this on its own.
export function publishRequest(id) {
  return { path: `/api/intake/${encodeURIComponent(id)}/publish`, method: "POST", body: {} };
}

/// A card's `outbound` record (`#171`), shaped for the modal: its state, the
/// comment link once posted, and any labels the repository does not have or
/// the error from the last attempt. Null for a card with nothing outbound
/// yet.
export function outboundInfo(card) {
  const o = card && card.outbound;
  if (!o) return null;
  return {
    state: o.state,
    commentUrl: o.comment_url || null,
    labelsApplied: o.labels_applied || [],
    labelsSkipped: o.labels_skipped || [],
    error: o.last_error || null,
  };
}

export const OUTBOUND_STATE_LABELS = {
  awaiting_approval: "awaiting approval",
  published: "published to GitHub",
  failed: "GitHub publish failed",
};

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
