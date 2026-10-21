//! `#339`: a run's proposed change to one of its scope's skills, as the
//! Inbox lists it -- the summary, the run it came from, the diff to read,
//! and the owner's two answers. Pure: the daemon decides what is open
//! (`GET /api/skills/proposals?state=open`); this only shapes it.

export const SKILL_PROPOSALS_PATH = "/api/skills/proposals?state=open";

export const SKILL_ACTION_LABELS = Object.freeze({ accept_skill: "Accept", reject_skill: "Reject" });

/// Whether a button's action is one of these, not a run's.
export function isSkillAction(action) {
  return Object.hasOwn(SKILL_ACTION_LABELS, action);
}

/// Seconds since `filedAt`, never negative; 0 for an unreadable time.
function ageOf(filedAt, nowMs) {
  const at = Date.parse(filedAt || "");
  return Number.isFinite(at) ? Math.max(0, Math.round((nowMs - at) / 1000)) : 0;
}

function changedFiles(proposal) {
  return (proposal.changes || []).map(change => change.kind === "removed" ? `−${change.path}` : change.path).join(", ");
}

/// One Inbox row per open proposal, oldest first: a person decides them in
/// the order they were filed. `report` is the endpoint's answer, or `null`
/// when it could not be read -- which contributes nothing, never an error.
export function skillInboxRows(report, nowMs = Date.now()) {
  return (report?.proposals || [])
    .filter(p => p.state === "open")
    .slice()
    .sort((a, b) => String(a.filed_at).localeCompare(String(b.filed_at)))
    .map(p => ({
      id: `skill:${p.id}`,
      kind: "skill_proposal",
      proposal: p.id,
      tone: "amber",
      title: `skill ${p.skill}`,
      label: "change proposed",
      reason: `${p.summary} · ${changedFiles(p)} · from ${p.origin?.agent || "a run"}`,
      scope: p.scope,
      age: ageOf(p.filed_at, nowMs),
      task_id: p.origin?.task_id,
      run_id: p.origin?.run_id,
      diff: p.diff || "",
      actions: ["accept_skill", "reject_skill"],
    }));
}

/// The request an Inbox button sends, or `null` for one this does not own.
/// Rejecting needs a reason; accepting needs nothing but the click.
export function skillActionRequest(action, proposal, { reason } = {}) {
  const id = encodeURIComponent(proposal);
  if (action === "accept_skill") return { method: "POST", path: `/api/skills/proposals/${id}/accept`, body: {} };
  if (action === "reject_skill") return { method: "POST", path: `/api/skills/proposals/${id}/reject`, body: { reason } };
  return null;
}
