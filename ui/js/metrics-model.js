//! Pure shaping logic for the L5 Improvement Metrics tab (`#333`): which
//! scope to ask for, one display row per listed metric (its value, freshness,
//! source and consumers joined from one `GET /api/metrics?scope=` answer),
//! labels, filtering and the L2 Dependencies link for a service source. The
//! daemon decides which metrics a scope is measured by and judges every
//! consumer itself (`metrics_catalogue::rows`); nothing here re-judges a
//! value -- the same split `quality.js`/`quality-model.js` keep, so a Node
//! test can exercise every rule with no DOM at all.

/// A scope path's segments below the instance root -- `scopes.js`'s own
/// reading, kept here pure so the root scope can be found without the DOM.
function segments(path, root) {
  let p = String(path ?? "");
  const r = String(root ?? "").replace(/\/+$/, "");
  if (r && (p === r || p.startsWith(`${r}/`))) p = p.slice(r.length);
  return p.split("/").filter((s) => s !== "" && s !== ".");
}

/// The scope a listing is asked for: the rail's selection, or -- with every
/// scope selected -- the scope that sits on the instance root, whose subtree
/// is everything. `null` when there is neither, and the view asks for a
/// selection instead of inventing an answer.
export function listingScope(selected, scopes, root) {
  if (selected) return selected;
  const top = (scopes || []).find((s) => segments(s.path, root).length === 0);
  return top ? top.name : null;
}

export function metricsUrl(scope) {
  return `/api/metrics?scope=${encodeURIComponent(scope)}`;
}

/// One display row per `rows[]` entry, in the daemon's order, with the
/// value, reason and `as_of` from `values[]` and title, unit and direction
/// from `registry[]`.
export function shapeRows(answer) {
  if (!answer) return [];
  const values = new Map((answer.values || []).map((v) => [v.id, v]));
  const defs = new Map((answer.registry || []).map((d) => [d.id, d]));
  return (answer.rows || []).map((row) => {
    const value = values.get(row.id) || {};
    const def = defs.get(row.id) || {};
    return {
      id: row.id,
      title: def.title || row.id,
      unit: def.unit || "",
      better: def.better || "",
      value: typeof value.value === "number" ? value.value : null,
      reason: value.reason || "",
      asOf: value.as_of || null,
      freshness: row.freshness || "no_data",
      maxAge: row.max_age || null,
      source: row.source || { kind: "built_in" },
      consumers: row.consumers || [],
    };
  });
}

/// A value in its unit's own terms: a ratio as a percentage, a count as a
/// whole number when it is one; `None` for no value, never a manufactured 0.
export function formatValue(value, unit) {
  if (value === null || value === undefined || !Number.isFinite(value)) return "None";
  if (unit === "ratio") return `${(value * 100).toFixed(1)}%`;
  if (Number.isInteger(value)) return String(value);
  return value.toFixed(2);
}

export function betterLabel(better) {
  return better === "higher" ? "↑ higher is better" : better === "lower" ? "↓ lower is better" : "";
}

const FRESHNESS = {
  fresh: "fresh",
  stale: "stale",
  no_data: "no data",
  no_window: "no max_age",
};

export function freshnessLabel(freshness) {
  return FRESHNESS[freshness] || freshness || "";
}

/// The badge class a freshness reads as -- the shared `.badge s-*` colours.
export function freshnessClass(freshness) {
  return { fresh: "met", stale: "stale", no_data: "no_data", no_window: "draft" }[freshness] || "no_data";
}

const STATUS = {
  met: "met",
  not_met: "not met",
  stale: "stale",
  no_data: "no data",
  draft: "draft",
};

export function statusLabel(status) {
  return STATUS[status] || status || "";
}

const KIND = { policy: "control", quality: "scenario", goal: "key result" };

export function consumerKindLabel(kind) {
  return KIND[kind] || kind || "";
}

/// What a consumer holds the value to, in its own bounds.
export function consumerBounds(consumer) {
  const parts = [];
  if (typeof consumer.above === "number") parts.push(`≥ ${consumer.above}`);
  if (typeof consumer.below === "number") parts.push(`≤ ${consumer.below}`);
  if (consumer.max_age) parts.push(`max_age ${consumer.max_age}`);
  return parts.join(" · ");
}

/// `<kind> <id> @ <scope>` -- how a consumer is named in a row.
export function consumerName(consumer) {
  const at = consumer.scope ? ` @ ${consumer.scope}` : "";
  return `${consumerKindLabel(consumer.kind)} ${consumer.id}${at}`;
}

/// `{ met: n, not_met: n, ... }` over a row's consumers.
export function statusCounts(consumers) {
  const counts = {};
  for (const c of consumers || []) counts[c.status] = (counts[c.status] || 0) + 1;
  return counts;
}

/// The source cell: a label, a detail line, and -- for anything a declared
/// service answers -- the service and the scope that declares it, for the
/// link to its row on L2 Dependencies.
export function sourceCell(source) {
  switch (source?.kind) {
    case "built_in":
      return { label: "built-in", detail: "computed from what Factory records", service: null };
    case "file":
      return { label: `file · ${source.source}`, detail: source.path, service: null, scope: source.scope };
    case "service":
      return {
        label: `service · ${source.service}`,
        detail: source.url,
        service: source.service,
        scope: source.scope,
      };
    case "unresolved":
      return {
        label: source.service ? `service · ${source.service} (refused)` : "unresolved",
        detail: source.reason,
        service: source.service || null,
        scope: source.scope || null,
        bad: true,
      };
    default:
      return { label: "", detail: "", service: null };
  }
}

/// Where a service source's link goes: its declaring scope's L2
/// Dependencies tab, Services segment, that service's row.
export function dependencyTarget(source) {
  const cell = sourceCell(source);
  if (!cell.service || !cell.scope) return null;
  return { scope: cell.scope, service: cell.service };
}

/// Rows whose id, title, source or a consumer mention every word of `query`.
export function filterRows(rows, query) {
  const words = String(query || "").toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return rows;
  return rows.filter((row) => {
    const cell = sourceCell(row.source);
    const hay = [
      row.id,
      row.title,
      cell.label,
      cell.detail,
      ...row.consumers.map((c) => consumerName(c)),
    ]
      .join(" ")
      .toLowerCase();
    return words.every((w) => hay.includes(w));
  });
}

/// `"2 metrics"`, `"1 metric"`.
export function countLine(rows) {
  const n = rows.length;
  return n === 1 ? "1 metric" : `${n} metrics`;
}
