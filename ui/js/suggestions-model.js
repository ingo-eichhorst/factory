//! Pure shaping logic for the L5 Improvement Suggestions tab (`#275`):
//! kind/state labels, display formatting, a group's summary line, button-
//! visibility predicates, request bodies and links. The daemon computes
//! the filtered list and its target groups fresh on every call
//! (`GET /api/suggestions`); this file only shapes what is already there
//! for the DOM -- the same split `quality.js`/`quality-model.js` keep, so a
//! Node test can exercise every rule here with no DOM at all.

import { refLinks } from "./policy-model.js";

export const KINDS = ["capability", "docs", "spec", "process", "entity", "other"];
export const STATES = ["open", "tasked", "dismissed", "done"];

const KIND_LABELS = {
  capability: "Capability",
  docs: "Docs",
  spec: "Spec",
  process: "Process",
  entity: "Entity",
  other: "Other",
};

export function kindLabel(kind) {
  return KIND_LABELS[kind] || kind || "";
}

const STATE_LABELS = {
  open: "Open",
  tasked: "Tasked",
  dismissed: "Dismissed",
  done: "Done",
};

export function stateLabel(state) {
  return STATE_LABELS[state] || state || "";
}

/// A suggestion's state only ever moves forward, the same rule
/// `factory_assurance::suggestion::Suggestion`'s own state machine
/// enforces server-side -- `dismissed` and `done` are both terminal. This
/// is only what the buttons show; the daemon remains the actual guard.
export function isTerminal(state) {
  return state === "dismissed" || state === "done";
}
export function canTask(s) {
  return !isTerminal(s.state);
}
export function canDismiss(s) {
  return !isTerminal(s.state);
}
export function canDone(s) {
  return !isTerminal(s.state);
}
/// "Ask the agent" needs a recorded session to resume -- without one the
/// daemon would refuse outright (`#275`'s "never silently start a fresh
/// session"), so the button does not offer what cannot work.
export function canAsk(s) {
  return !!s.session_id && !isTerminal(s.state);
}

export function formatTokens(n) {
  if (n === null || n === undefined) return "?";
  if (n >= 10000) return `${Math.round(n / 1000)}k`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
}

/// `null`/`undefined` is unknown, never free -- the same "no data is not
/// zero" rule the rest of this app's cost displays follow.
export function formatCost(usd) {
  if (usd === null || usd === undefined) return null;
  if (usd === 0) return "$0";
  // Below a cent, two decimals would read as free; show a third instead.
  return `$${usd.toFixed(Math.abs(usd) < 0.01 ? 3 : 2)}`;
}

/// The prioritisation signal the issue asks for: how many, how many are
/// still open, and the summed cost -- "web access to X blocked" reported
/// by ten runs reads as one line, not ten.
export function groupSummaryLine(group) {
  const parts = [`${group.count} suggestion${group.count === 1 ? "" : "s"}`];
  parts.push(group.open_count === group.count ? "all open" : `${group.open_count} open`);
  const bits = [];
  if (group.total_wasted_tokens) bits.push(`~${formatTokens(group.total_wasted_tokens)} tokens claimed`);
  if (group.total_cost_usd) bits.push(`${formatCost(group.total_cost_usd)} recorded`);
  if (bits.length) parts.push(bits.join(", "));
  return parts.join(" · ");
}

/// A free-text filter over target/summary/detail/scope/agent -- the one
/// axis `GET /api/suggestions` does not offer, since it is a convenience
/// over what the server already sent, not a second source of truth.
export function matchesQuery(suggestion, query) {
  const q = (query || "").trim().toLowerCase();
  if (!q) return true;
  return [suggestion.target, suggestion.summary, suggestion.detail, suggestion.scope, suggestion.agent]
    .filter(Boolean)
    .some((field) => field.toLowerCase().includes(q));
}

export function filterByQuery(suggestions, query) {
  return suggestions.filter((s) => matchesQuery(s, query));
}

/// The groups to actually draw: the server's own `groups`, each narrowed
/// to the ids a text query still matches, with a group a query emptied out
/// entirely dropped -- so searching narrows groups, not just rows inside
/// an unchanged set of them. Never mutates `report`.
export function visibleGroups(report, query) {
  if (!report) return [];
  if (!query || !query.trim()) return report.groups;
  const keep = new Set(filterByQuery(report.suggestions, query).map((s) => s.id));
  return report.groups
    .map((g) => ({ ...g, ids: g.ids.filter((id) => keep.has(id)) }))
    .filter((g) => g.ids.length > 0);
}

export function suggestionById(report, id) {
  return ((report && report.suggestions) || []).find((s) => s.id === id) || null;
}

/// The suggestions a group's own ids name, in the group's order.
export function groupSuggestions(report, group) {
  return group.ids.map((id) => suggestionById(report, id)).filter(Boolean);
}

/// A short, stable id for display -- the same eight-character convention
/// this app already uses for a task/attestation id elsewhere.
export function shortId(id) {
  return (id || "").slice(0, 8);
}

export function dismissBody(reason) {
  return { reason };
}
export function taskBody(ids) {
  return { ids };
}
export function askBody(question) {
  return { question };
}

/// "Dismiss" asks for a real reason before it acts -- checked here, before
/// the request goes out, rather than only after the daemon refuses an
/// empty one.
export function validDismissReason(reason) {
  return !!(reason && reason.trim());
}
export function validQuestion(question) {
  return !!(question && question.trim());
}

/// A task-modal link for the task a suggestion was filed against, and
/// (when a run id is given) that exact run within it -- the same
/// `refLinks` door `quality.js`'s `taskHref` opens, so a suggestion's link
/// can never drift from the one a quality scenario's own task ref draws.
export function suggestionTaskHref(scope, taskId) {
  return refLinks(scope, [{ kind: "task", id: taskId }])[0].href;
}

export function suggestionRunHref(scope, taskId, runId) {
  const links = refLinks(scope, [{ kind: "task", id: taskId }, { kind: "run", id: runId }]);
  return (links[1] || links[0]).href;
}
