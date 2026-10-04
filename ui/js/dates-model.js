//! Important dates are composed by the daemon from producer-owned metadata
//! and native clocks. These helpers only present that answer, never probe,
//! renew, reschedule a task, or duplicate the legal clock's Inbox logic.
export function datesQuery(scope) {
  return `/api/important-dates${scope ? `?${new URLSearchParams({ scope })}` : ""}`;
}

export function expiryText(entry) {
  const item = entry?.observation;
  if (!item) return "unknown";
  if (item.no_expiry) return item.source === "github_auth" ? "no reported expiry" : "no expiry";
  return item.expires_at || "unknown";
}

export function stateText(entry) {
  if (entry.resolved) return "resolved";
  return ({ due_soon: "due soon", ok: "ok", overdue: "overdue", unknown: "unknown" })[entry.state] || "unknown";
}

export function dateTone(entry) {
  if (entry.resolved) return "ok";
  if (entry.milestone === "expired" || entry.state === "overdue") return "bad";
  if (entry.milestone || entry.state === "due_soon") return "warn";
  return entry.state === "unknown" ? "warn" : "ok";
}

export function dependencyText(entry) {
  return (entry.observation.affects || []).map(dep => dep.label).join(", ") || "instance; no specific dependant declared";
}

const MILESTONES = Object.freeze({ lead: "renewal lead time", seven_days: "expires within 7 days", one_day: "expires within 1 day", expired: "expired", scheduled_run: "expires before a scheduled run" });

export function renewalInboxRows(report) {
  return (report?.entries || []).filter(entry => !entry.resolved && entry.milestone && !["policy_attestation", "cra_deadline"].includes(entry.observation.source)).map(entry => {
    const item = entry.observation;
    const risk = (entry.scheduled_risks || []).map(run => `${run.scope}/${run.agent}: ${run.title} at ${run.next_run_at}`).join("; ");
    return {
      id: `renewal:${item.id}`, kind: "renewal", tone: dateTone(entry) === "bad" ? "red" : "amber",
      title: item.name, label: MILESTONES[entry.milestone] || "renewal needed", age: 0,
      scope: item.scope, scopes: [...new Set((item.affects || []).map(dep => dep.scope).filter(Boolean))],
      reason: `${expiryText(entry)} · ${item.basis}${item.issue ? " (last known; observation unavailable)" : ""} · depends: ${dependencyText(entry)}${risk ? ` · scheduled: ${risk}` : ""} · renew: ${item.renew} · owner: ${item.owner}`,
      href: entry.href || "#all/dates", actions: [],
    };
  });
}

export function dependencyWarnings(report, { scope, agent, environment }) {
  return (report?.entries || []).filter(entry => !entry.resolved && (entry.milestone || ["due_soon", "overdue"].includes(entry.state)) && entry.observation.affects.some(dep => {
    if (dep.scope !== scope) return false;
    if (environment) return dep.environment === environment || (!dep.environment && !dep.agent && !dep.provider);
    return dep.agent === agent || (!dep.agent && !dep.environment && !dep.provider);
  }));
}

export function datesSummary(report) {
  if (!report) return { text: "not available right now", counts: "", next: "unknown" };
  return { text: "Important dates", counts: `${report.due_soon} due soon · ${report.overdue} overdue · ${report.unknown} unknown`, next: report.next_expiry || "none known" };
}
