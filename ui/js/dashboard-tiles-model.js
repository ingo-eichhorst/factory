//! Pure shaping for #162 (phase 3 of #150): the dashboard's registry metric
//! tiles, and the richer view tiles that each read one existing endpoint
//! shared by every tile that needs it -- `/api/metrics`, `/api/occupancy`,
//! `/api/policy`, `/api/costs`. No DOM: `dashboard.js` is the only module
//! that draws, the same boundary `bench-model.js`/`policy-model.js` already
//! keep, and this file is safe to import in a Node test with no page at all.
//! `formatUnitValue`/`metricUnavailableNote` (a metric value's own unit and
//! "not measured yet" copy) are `goals-model.js`'s, reused rather than
//! reimplemented; `frameworkCards` is `policy-model.js`'s.
//!
//! ## Which endpoints a layout needs
//!
//! A tile only ever reads the one shared answer its kind needs -- never a
//! fetch of its own -- so `neededEndpoints`/`neededMetricIds` below are what
//! `dashboard.js` asks before it fetches anything beyond `/api/production`:
//! the default layout (`DEFAULT_DASHBOARD`, #163) has no `metric` tile and
//! none of #150's newer `view` tiles, so both read as "nothing new needed"
//! and the default page starts no request this phase did not already make.

import { tileKind } from "./dashboard-model.js";
import { formatUnitValue, metricUnavailableNote } from "./goals-model.js";
import { frameworkCards } from "./policy-model.js";
import { inScope } from "./scopes.js";

/// View ids whose card reads `/api/occupancy` -- the one shared read #150
/// gives all three: two roll it up (by scope, by agent), one draws it as a
/// compact per-agent strip.
const OCCUPANCY_VIEWS = Object.freeze(["agent_hours_by_scope", "agent_hours_by_agent", "occupancy_strip"]);

/// Every metric id a `metric` tile in `tiles` names, deduplicated and
/// sorted -- one `/api/metrics?ids=` request covers every metric tile on
/// the page, never one per tile. Empty when there is none: `/api/metrics`
/// with no `ids` means "every non-parameterised metric" on the wire, so an
/// empty list here must never reach the query string as `ids=` -- the
/// caller's job, not this function's, but the reason this stays a plain
/// array rather than folding "no metric tiles" and "every metric" together.
export function neededMetricIds(tiles) {
  const ids = new Set();
  for (const tile of tiles || []) {
    if (tileKind(tile) === "metric" && typeof tile.metric === "string" && tile.metric.trim()) {
      ids.add(tile.metric.trim());
    }
  }
  return [...ids].sort();
}

/// Which of the four new shared reads (`/api/metrics` beyond ids, `/api/
/// occupancy`, `/api/operations`, `/api/policy`, `/api/costs`) the resolved
/// layout actually needs -- read at most once per render cycle, and only
/// when some tile on the layout needs it. `DEFAULT_DASHBOARD` needs none of
/// these: every flag here reads `false` for it, which is what keeps the
/// hard requirement that the default page start no new fetch.
export function neededEndpoints(tiles) {
  const need = { metrics: false, occupancy: false, operations: false, policy: false, costs: false };
  for (const tile of tiles || []) {
    const kind = tileKind(tile);
    if (kind === "metric") {
      if (typeof tile.metric === "string" && tile.metric.trim()) need.metrics = true;
      continue;
    }
    if (kind !== "view") continue;
    if (OCCUPANCY_VIEWS.includes(tile.view)) need.occupancy = true;
    else if (tile.view === "inbox") need.operations = true;
    else if (tile.view === "compliance") need.policy = true;
    else if (tile.view === "cost") need.costs = true;
  }
  return need;
}

// ------------------------------------------------------------- metric tiles

/// `↑`/`↓` for `MetricDef.better` (`"higher"`/`"lower"`), `""` for anything
/// else (a definition this page has not fetched yet). Display only -- never
/// a claim about which way a series is actually moving, which is what a
/// sparkline is for.
export function betterGlyph(better) {
  if (better === "higher") return "↑";
  if (better === "lower") return "↓";
  return "";
}

/// The qualifier a metric tile adds when its own definition says it never
/// followed the asked scope to begin with (`bench.*`, `goal_tasks_done.*`
/// today) -- `""` for `"scope_aware"` and for a missing/unrecognised field,
/// so a build running against `main` before #182 merges its `coverage`
/// field reads this as "nothing to add" rather than crashing or guessing.
export function coverageQualifier(coverage) {
  return coverage === "instance_wide" ? "instance-wide" : "";
}

/// One `metric` tile's whole content, off a `/api/metrics` answer
/// (`{values, series, registry}`, or `null`/`undefined` for "not fetched" or
/// "the fetch failed") and the `MetricId` the tile names. Never throws on a
/// shape this page has not seen: an id missing from the answer (the fetch
/// failed, or raced a layout change) reads the same as "not available right
/// now", the same honest fallback `metricUnavailableNote` already gives a
/// metric the daemon itself could not compute.
export function metricTileView(id, answer) {
  const values = (answer && answer.values) || [];
  const registry = (answer && answer.registry) || [];
  const series = (answer && answer.series) || [];
  const def = registry.find((d) => d.id === id) || null;
  const mv = values.find((v) => v.id === id) || null;
  const s = series.find((sr) => sr.id === id) || null;
  const reason = mv ? metricUnavailableNote(mv) : "not available right now";
  const points = s && Array.isArray(s.points) ? s.points.map(([, v]) => v) : null;
  return {
    id,
    title: def ? def.title : id,
    unavailable: !!reason,
    reason,
    valueText: reason ? null : formatUnitValue(mv.value, def && def.unit),
    better: def ? betterGlyph(def.better) : "",
    coverage: def ? coverageQualifier(def.coverage) : "",
    // A flat line at zero reads as "a stable trend of zero", which is not
    // the same fact as "nothing has happened here yet" -- `dashboard.js`'s
    // own `kpis()` already refuses to draw a sparkline for that reason
    // (`everFinished`); a metric tile has no such context to check, so it
    // applies the same rule directly to the series it was handed.
    points: points && points.some((v) => v !== 0) ? points : null,
  };
}

/// Whole days between two RFC 3339 timestamps, rounded -- what an
/// occupancy or cost answer's own `from`/`now`/`to` becomes in a tile's
/// qualifier when the dashboard's requested window (`day`/`14d`/`90d`) may
/// not be the window the answer actually covers (`/api/occupancy` clamps at
/// 30 days; `/api/costs` may answer a shorter span than asked for a young
/// instance). `null` for anything that does not parse, so a caller can fall
/// back to no qualifier rather than showing "NaN days".
export function windowDays(fromIso, toIso) {
  const from = Date.parse(fromIso);
  const to = Date.parse(toIso);
  if (!Number.isFinite(from) || !Number.isFinite(to)) return null;
  return Math.round((to - from) / 86400000);
}

// --------------------------------------------------------- agent hours (#150)

/// `busy_seconds`/`blocked_seconds` (`/api/occupancy`'s own fields, #120/
/// #121) summed per scope, hours -- one row per in-scope scope the
/// occupancy answer named, even one with nothing recorded: a quiet scope is
/// itself the fact "agent hours by scope" is answering. Blocked hours are
/// carried beside busy hours, never subtracted from them -- the same rule
/// `occupancy-model.js`'s `waitingShare` already keeps for the chart.
/// Busiest first, ties broken by name for a stable order across redraws.
export function agentHoursByScope(occ) {
  if (!occ || !Array.isArray(occ.scopes)) return [];
  const rows = occ.scopes
    .filter((sc) => inScope(sc.name))
    .map((sc) => {
      const rows = sc.rows || [];
      const busy = rows.reduce((sum, r) => sum + (Number(r.busy_seconds) || 0), 0);
      const blocked = rows.reduce((sum, r) => sum + (Number(r.blocked_seconds) || 0), 0);
      return { key: sc.name, label: sc.name, busyHours: busy / 3600, blockedHours: blocked / 3600 };
    });
  rows.sort((a, b) => b.busyHours - a.busyHours || a.label.localeCompare(b.label));
  return rows;
}

/// The same figures, one row per agent instead of per scope -- `key` is
/// `scope/agent` since an agent's own name is only unique inside its scope
/// (`usage.rs`'s own `CostGroupBy::Agent` keeps the same rule).
export function agentHoursByAgent(occ) {
  if (!occ || !Array.isArray(occ.scopes)) return [];
  const rows = [];
  for (const sc of occ.scopes) {
    if (!inScope(sc.name)) continue;
    for (const r of sc.rows || []) {
      const busy = Number(r.busy_seconds) || 0;
      const blocked = Number(r.blocked_seconds) || 0;
      rows.push({ key: `${sc.name}/${r.agent}`, agent: r.agent, scope: sc.name, busyHours: busy / 3600, blockedHours: blocked / 3600 });
    }
  }
  rows.sort((a, b) => b.busyHours - a.busyHours || a.key.localeCompare(b.key));
  return rows;
}

/// A compact per-agent utilisation strip: `pct` of the answer's own window
/// (`occ.from` to the earlier of `occ.to`/`nowMs`) each agent's `busy_seconds`
/// covers -- the same fraction `occupancy.js`'s own util column reads off a
/// row, without the timeline underneath it. Busiest first.
export function occupancyStripRows(occ, nowMs) {
  if (!occ || !Array.isArray(occ.scopes)) return [];
  const from = Date.parse(occ.from);
  const to = Date.parse(occ.to);
  if (!Number.isFinite(from) || !Number.isFinite(to)) return [];
  const elapsed = Math.max(1, (Math.min(nowMs, to) - from) / 1000);
  const rows = [];
  for (const sc of occ.scopes) {
    if (!inScope(sc.name)) continue;
    for (const r of sc.rows || []) {
      const busy = Number(r.busy_seconds) || 0;
      const pct = Math.max(0, Math.min(100, Math.round((busy / elapsed) * 100)));
      rows.push({ key: `${sc.name}/${r.agent}`, agent: r.agent, scope: sc.name, pct });
    }
  }
  rows.sort((a, b) => b.pct - a.pct || a.key.localeCompare(b.key));
  return rows;
}

// ----------------------------------------------------------- compliance (#150)

/// Per-framework met/open, off `frameworkCards` (`policy-model.js`) -- the
/// same split `compliance_value`/`open_controls_value` compute server-side:
/// `met` is satisfied + attested + n/a, `open` is open + stale. A framework
/// made only of best practice (`counted === 0`) is left with `counted: 0`
/// rather than a division the caller would have to guard against too.
export function complianceSummaryRows(report) {
  if (!report) return [];
  return frameworkCards(report).map((card) => {
    const c = card.counts || {};
    return {
      framework: card.framework,
      title: card.title,
      compliant: card.compliant,
      met: (c.satisfied || 0) + (c.attested || 0) + (c.not_applicable || 0),
      open: (c.open || 0) + (c.stale || 0),
      counted: card.countedTotal,
    };
  });
}

// ----------------------------------------------------------------- cost (#150)

/// The most expensive `limit` rows of a `/api/costs` report -- already
/// sorted most-expensive-first on the wire (`CostReport::sort_rows`), so
/// this only ever truncates, never reorders.
export function topCostRows(report, limit = 5) {
  return ((report && report.rows) || []).slice(0, limit);
}
