//! Pure helpers for the L1 Operations tab (#185): the environments the
//! systems Factory builds are deployed to, what runs on each, whether it is
//! healthy and whether it meets its SLO.
//!
//! The same boundary `backup-model.js` keeps: nothing here touches the DOM
//! or imports a module that does, so `environments.test.js` imports every
//! function below from Node. `environments.js` is the rendering.
//!
//! The daemon decides every fact -- status, incidents, availability, error
//! budget, the DORA keys (`factory_core::environments`) -- and sends it as
//! data. These helpers only put words and colours to it. A figure the report
//! leaves `null` is shown as missing, never as zero: an environment nobody
//! checked has no availability, not 0% and not 100%.

import { MISSING } from "./infra-model.js";
import { fmtAgo, fmtSpan, fmtWhen } from "./backup-model.js";

export { MISSING, fmtAgo, fmtSpan, fmtWhen };

/// What a scope's config needs for the page to have anything to show: the
/// issue's own example, in the scope's `.factory/config.yaml`.
export const CONFIG_SNIPPET = `# a scope's .factory/config.yaml
scope:
  name: factory
  environments:
    - name: production
      tier: production
      url: https://factory.example.ts.net:8790
      checks:
        - { kind: http, path: /api/status, expect: 200, every: 60s, timeout: 5s }
      slo: { availability: 99.5%, window: 28d }
    - name: staging
      tier: staging
      promotes_to: production
      url: https://factory.example.ts.net:8791
      checks:
        - { kind: http, path: /api/status, expect: 200, every: 60s }
      slo: { availability: 99%, window: 28d }`;

/// The three-colour scale the page draws with, plus `none` for "nothing to
/// judge": ok (green), warn (amber), bad (red).
export function statusTone(status) {
  switch (status) {
    case "up": return "ok";
    case "degraded": return "warn";
    case "down": return "bad";
    default: return "none";
  }
}

export function statusLabel(status) {
  switch (status) {
    case "up": return "Up";
    case "degraded": return "Degraded";
    case "down": return "Down";
    default: return "Unknown";
  }
}

/// `up 3h 12m` / `down 4m`, or just the status when the report says no
/// since (unknown, paused).
export function statusText(card, now) {
  const label = statusLabel(card && card.status);
  if (card && card.paused) return `${label} · checks paused`;
  if (!card || !card.status_since) return label;
  return `${label} for ${fmtAgo(now, card.status_since).replace(/ ago$/, "")}`;
}

/// A ratio as a percentage with enough digits to tell 99.5% from 99.9%.
export function fmtPct(ratio) {
  if (typeof ratio !== "number" || !Number.isFinite(ratio)) return MISSING;
  const pct = ratio * 100;
  if (pct === 100 || pct === 0) return `${pct}%`;
  return `${pct.toFixed(pct >= 99 || pct <= -99 ? 2 : 1)}%`;
}

/// An uptime figure against the SLO target: `ok` at or above it, `bad`
/// below it, `none` with no figure or no target to judge by.
export function uptimeLevel(ratio, slo) {
  if (typeof ratio !== "number" || !slo || typeof slo.target !== "number") return "none";
  return ratio >= slo.target ? "ok" : "bad";
}

/// What is left of the error budget: `ok` more than a quarter left, `warn`
/// the last quarter, `bad` spent or overspent.
export function budgetLevel(budget) {
  if (typeof budget !== "number" || !Number.isFinite(budget)) return "none";
  if (budget <= 0) return "bad";
  if (budget < 0.25) return "warn";
  return "ok";
}

export function budgetText(budget) {
  if (typeof budget !== "number" || !Number.isFinite(budget)) return MISSING;
  if (budget < 0) return `overspent by ${fmtPct(-budget)}`;
  return `${fmtPct(budget)} left`;
}

export function sloText(slo) {
  if (!slo) return "no SLO declared";
  return `${fmtPct(slo.target)} over ${slo.window_days}d`;
}

export function tierLabel(tier) {
  switch (tier) {
    case "production": return "production";
    case "staging": return "staging";
    default: return "ephemeral";
  }
}

/// A commit shortened for a table cell.
export function shortCommit(commit) {
  return commit ? String(commit).slice(0, 10) : MISSING;
}

/// One release as a line: `v1.2 · 3a4b5c6d7e (dirty)`, the describe when
/// there is no version.
export function releaseText(release) {
  if (!release || !release.commit) return MISSING;
  const name = release.version || release.describe;
  const bits = [];
  if (name && name !== release.commit) bits.push(name);
  bits.push(shortCommit(release.commit));
  return bits.join(" · ") + (release.dirty ? " (dirty)" : "");
}

/// Who made a deployment: `owner (by hand)`, `builder · run 1a2b3c4d`,
/// `owner via release.sh`.
export function actorText(deployment) {
  const a = deployment && deployment.actor;
  if (!a) return MISSING;
  let text = a.name || MISSING;
  if (a.kind === "run" && a.run_id) text += ` · run ${String(a.run_id).slice(0, 8)}`;
  else if (a.kind === "agent") text += " (agent)";
  if (deployment.via) text += ` via ${deployment.via}`;
  if (deployment.manual) text += " (by hand)";
  return text;
}

export function deployStatusLabel(status) {
  switch (status) {
    case "running": return "running";
    case "succeeded": return "succeeded";
    case "failed": return "failed";
    case "rolled_back": return "rolled back";
    default: return String(status || MISSING);
  }
}

/// `ok`, `warn` (running) or `bad` (failed, rolled back): the two outcomes
/// that must stand out in the timeline.
export function deployTone(status) {
  switch (status) {
    case "succeeded": return "ok";
    case "running": return "warn";
    case "failed":
    case "rolled_back": return "bad";
    default: return "none";
  }
}

/// A duration: finished ones from the report's own two instants, a running
/// one against `now`.
export function durationText(deployment, now) {
  if (!deployment || !deployment.started_at) return MISSING;
  const end = deployment.finished_at || now;
  const a = new Date(deployment.started_at).getTime();
  const b = new Date(end).getTime();
  if (!Number.isFinite(a) || !Number.isFinite(b)) return MISSING;
  const text = fmtSpan(Math.max(0, (b - a) / 1000));
  return deployment.finished_at ? text : `${text} so far`;
}

/// The post-deploy verification in a word and its detail: which checks
/// failed and what they said.
export function verificationText(deployment) {
  const v = deployment && deployment.verification;
  if (!v) {
    if (!deployment || deployment.status === "running") return { level: "none", text: MISSING };
    return { level: "none", text: "not verified" };
  }
  if (v.ok) return { level: "ok", text: `passed · ${(v.checks || []).length} check${(v.checks || []).length === 1 ? "" : "s"}` };
  const failing = (v.checks || []).filter(c => !c.ok).map(c => `${c.check}: ${c.detail || "failed"}`);
  return { level: "bad", text: `FAILED · ${failing.join("; ") || "a check failed"}` };
}

/// The timeline as the report orders it (running first, then newest), each
/// row with its words already chosen.
export function deploymentRows(report) {
  const now = report && report.generated_at;
  return ((report && report.deployments) || []).map(d => ({
    id: d.id,
    environment: d.environment,
    scope: d.scope,
    release: releaseText(d.release),
    commit: d.release && d.release.commit,
    status: deployStatusLabel(d.status),
    tone: deployTone(d.status),
    standsOut: d.status === "failed" || d.status === "rolled_back",
    duration: durationText(d, now),
    who: actorText(d),
    when: fmtWhen(d.started_at),
    reason: d.reason || "",
    previous: d.previous_commit ? shortCommit(d.previous_commit) : null,
    verification: verificationText(d),
  }));
}

/// The catalogue, one row per release as the report sends it, with where it
/// runs and how its deployments went.
export function releaseRows(report) {
  return ((report && report.releases) || []).map(r => ({
    scope: r.scope,
    commit: shortCommit(r.commit),
    name: r.version || r.describe || "",
    dirty: !!r.dirty,
    firstSeen: fmtWhen(r.first_seen),
    runningOn: r.running_on || [],
    deployments: r.deployments || 0,
    failed: r.failed_deployments || 0,
  }));
}

/// Only the verified current release can be promoted; readiness is decided
/// by the daemon, including targets outside the rail's selected subtree.
export function promotionChoices(report, release) {
  if (!release) return [];
  return (report?.environments || []).filter(card => card.promotion_ready && card.promotes_to
    && card.current?.release?.commit === release.commit && card.current?.scope === release.scope)
    .map(card => ({ source: card.name, target: card.promotes_to, deployment: card.current.id }));
}

export function bucketHasIncident(bucket, check, incidents) {
  const from = Date.parse(bucket.start);
  const to = from + 30 * 60 * 1000;
  return (incidents || []).some(incident => incident.checks.includes(check.name)
    && Date.parse(incident.started_at) < to && (!incident.ended_at || Date.parse(incident.ended_at) > from));
}

export function samplesSelection(report, environment, check, start = null) {
  const to = new Date(report.generated_at);
  const from = start ? new Date(start) : new Date(to.getTime() - 24 * 60 * 60 * 1000);
  const end = start ? new Date(Math.min(to.getTime(), from.getTime() + 30 * 60 * 1000)) : to;
  return { environment, check, from: from.toISOString(), to: end.toISOString() };
}

export function samplesQuery(selection, scope, before = null) {
  const query = new URLSearchParams({ ...selection, limit: "200" });
  if (scope) query.set("scope", scope);
  if (before != null) query.set("before", String(before));
  return `/api/environments/samples?${query}`;
}

export function recoveryStatus(action) {
  if (action.status === "done") return "completed";
  if (action.status === "cancelled") return "cancelled";
  return action.run?.status || action.status || "pending";
}

/// A strip slot's tone: `ok` all fast, `warn` any slow, `bad` any failure, `none` not
/// checked in that half hour.
export function bucketTone(bucket) {
  if (!bucket) return "none";
  if (bucket.failed > 0) return "bad";
  if (bucket.slow > 0) return "warn";
  if (bucket.ok > 0) return "ok";
  return "none";
}

/// A check's newest answer in a line: `200 · 42ms · 3m ago`.
export function lastCheckText(check, now) {
  const last = check && check.last;
  if (!last) return "not checked yet";
  const bits = [last.ok ? (last.slow ? "slow" : "ok") : "failing"];
  if (last.detail) bits.push(last.detail);
  if (typeof last.latency_ms === "number") bits.push(`${last.latency_ms}ms`);
  bits.push(fmtAgo(now, last.at));
  return bits.join(" · ");
}

/// An incident in a line: when it started, how long, which checks.
export function incidentText(incident, now) {
  if (!incident) return MISSING;
  const end = incident.ended_at || now;
  const secs = (new Date(end).getTime() - new Date(incident.started_at).getTime()) / 1000;
  const span = Number.isFinite(secs) ? fmtSpan(Math.max(0, secs)) : MISSING;
  const checks = (incident.checks || []).join(", ");
  const state = incident.ended_at ? `lasted ${span}` : `open for ${span}`;
  return `${fmtWhen(incident.started_at)} · ${state}${checks ? ` · ${checks}` : ""}`;
}

/// The four DORA keys and MTTR as label/value pairs, each missing when the
/// report could not compute it.
export function doraRows(dora) {
  const d = dora || {};
  const secs = (v) => (typeof v === "number" ? fmtSpan(v) : MISSING);
  return [
    { label: "Deploys / week", value: typeof d.deploy_frequency === "number" ? d.deploy_frequency.toFixed(1) : MISSING },
    { label: "Lead time p50", value: secs(d.lead_time_p50) },
    { label: "Change failure rate", value: fmtPct(d.change_failure_rate) },
    { label: "Time to restore p50", value: secs(d.time_to_restore_p50) },
    { label: "MTTR", value: secs(d.mttr) },
  ];
}

/// What went wrong fetching: a daemon built before the endpoint answers a
/// bare 404, which is not a fault.
export function environmentsFailure(error) {
  const message = error && error.message ? String(error.message) : String(error ?? "");
  return /^404\b/.test(message) ? "unavailable" : "error";
}

/// The events that change what the page shows. A health sample is never
/// one -- only a change of status is published.
export function isEnvironmentsEvent(ev) {
  return !!ev && (ev.type === "deployment_updated" || ev.type === "environment_status_changed");
}

/// The query string for the rail's selection: nothing for every scope.
export function environmentsQuery(scope) {
  return scope === null || scope === undefined ? "" : `?scope=${encodeURIComponent(scope)}`;
}
