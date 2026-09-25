//! A task's standing on the board (#122), pure: which column it sits in,
//! the one line that says why when its status alone does not, and which of
//! the modal's actions it allows. `tasks.js` draws; this decides.
//!
//! A *run* that failed stays `failed`. Its *task* goes to `blocked`, with
//! `failure` saying why -- a failure is something a person has to look at,
//! and Blocked is the one column that means that. Closed is `done` or
//! `cancelled`, and a failure never gets there on its own: closing one is a
//! person's act, with a reason (`closure`).

/// A task's closed statuses. Not `core.js`'s `TERMINAL`, which is about
/// runs -- a run still ends `failed`, a task no longer does.
export const CLOSED = ["done", "cancelled"];

/// The reasons a task is closed with, in the order the form offers them.
export const CLOSE_REASONS = [
  { key: "not_planned", label: "won't do" },
  { key: "completed", label: "completed" },
  { key: "duplicate", label: "duplicate" },
];

/// Why a run failed, as a person reads it. `FailKind` on the wire.
const FAIL_KINDS = {
  agent_failed: "the agent reported failed",
  session_gone: "its session went away",
  turn_ended: "its turn ended with no report",
  stop_failure: "its turn was cut short by an API error",
  ack_timeout: "never acknowledged",
  run_timeout: "ran past its timeout",
  blocked_timeout: "sat blocked with nobody answering",
  dispatch_failed: "dispatch failed",
};

export function failKindLabel(kind) {
  if (!kind) return "unclassified";
  return FAIL_KINDS[kind] || kind.replaceAll("_", " ");
}

export function closeReasonLabel(reason) {
  const known = CLOSE_REASONS.find((r) => r.key === reason);
  return known ? known.label : reason;
}

/// Blocked because its newest run failed, not because an agent is waiting
/// on a question: nobody to answer, only a failure to look at.
export function blockedByFailure(t) {
  return !!t && t.status === "blocked" && !!t.failure;
}

/// Ended in failure: blocked by one, or a row stored as `failed` before
/// #122 that the daemon has not moved yet.
export function hasFailed(t) {
  return blockedByFailure(t) || (!!t && t.status === "failed");
}

export function isClosed(t) {
  return !!t && CLOSED.includes(t.status);
}

/// Nothing is running and nothing will until someone acts.
export function isSettled(t) {
  return isClosed(t) || hasFailed(t);
}

/// How a closed task was closed: its close record, or what its status says
/// -- done is completed, cancelled is won't do. `null` when not closed.
export function closeReason(t) {
  if (!isClosed(t)) return null;
  if (t.closure) return t.closure.reason;
  return t.status === "done" ? "completed" : "not_planned";
}

/// The board's column. Blocked is the one a person has to act on, and a
/// failure goes there -- never to Closed, which holds only what was done or
/// deliberately closed. A legacy `failed` row goes there too.
export function columnFor(t) {
  if (t.status === "blocked" || t.status === "failed") return "blocked";
  if (t.status === "pending") return t.schedule ? "scheduled" : "manual";
  // `verifying` is still in progress: the agent said done and the daemon is
  // running the steps its control plan requires (`#118`).
  if (t.status === "dispatching" || t.status === "running" || t.status === "verifying") return "active";
  return "closed";
}

/// The line under a task's status when the status alone does not say
/// enough: the failure it is blocked on, the failure a queued retry is
/// retrying, or how it was closed. `{ tone, text, detail }` or `null`.
/// `tone` is `fault`, `wait` or `closed`; `detail` is the last error or
/// the closer's note, when there is one.
export function standing(t) {
  if (!t) return null;
  if (hasFailed(t)) {
    const f = t.failure || {};
    const attempt = f.attempt ? `attempt ${f.attempt} ` : "last attempt ";
    return { tone: "fault", text: `${attempt}failed: ${failKindLabel(f.kind)}`, detail: t.error || null };
  }
  // Keyed off the queued retry, never off `failure` alone: a pending task
  // with a failure and no retry is not mid-retry.
  if (t.status === "pending" && t.pending_retry) {
    const kind = t.failure ? failKindLabel(t.failure.kind) : "a failure";
    return { tone: "wait", text: `retrying after ${kind} (retry ${t.pending_retry.attempts})`, detail: t.error || null };
  }
  if (t.status === "done" && t.routed_to) {
    return { tone: "closed", text: `done → ${t.routed_to}`, detail: null };
  }
  const reason = closeReason(t);
  if (reason) {
    const c = t.closure || {};
    const text = c.duplicate_of ? `duplicate of ${c.duplicate_of.slice(0, 8)}` : closeReasonLabel(reason);
    return { tone: "closed", text, detail: c.note || null };
  }
  return null;
}

/// Which of the modal's task actions apply. Run and Cancel follow the
/// active run as before; Close is for a task with none (a failure waits for
/// exactly this) that is not closed already and not still in intake;
/// Reopen is for a closed one, except intake's `wontfix`, which intake
/// decides again.
export function taskActions(t, activeRun) {
  const active = !!activeRun;
  const intakeWontfix = t?.intake?.stage === "wontfix";
  return {
    run: !active,
    cancel: active,
    close: !active && !!t && !isClosed(t) && t.status !== "intake",
    reopen: !active && isClosed(t) && !intakeWontfix,
  };
}

/// The body `POST /api/tasks/{id}/close` takes, from the form's fields.
/// `duplicate_of` goes only with `duplicate`; empty strings are left out.
export function closeBody(reason, duplicateOf, note) {
  const body = { reason };
  const of = (duplicateOf || "").trim();
  if (reason === "duplicate" && of) body.duplicate_of = of;
  const n = (note || "").trim();
  if (n) body.note = n;
  return body;
}
