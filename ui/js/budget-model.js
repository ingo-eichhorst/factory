//! L6 Budget shaping only. Verdicts belong to the daemon; no price guesses.
export const GROUPS = [
  ["scope", "Scope"], ["agent", "Agent"], ["issue", "Issue"],
  ["workflow", "Workflow"], ["provider", "Provider account"],
];

export function budgetUrl(scope, group = "scope") {
  const q = new URLSearchParams({ group_by: group });
  if (scope) q.set("scope", scope);
  return `/api/budget?${q}`;
}

export function usd(value) {
  return typeof value === "number" && Number.isFinite(value)
    ? `$${value.toFixed(2)}` : "unknown";
}

export function uncertainty(row, unattributed = 0) {
  const notes = [];
  if (row.runs_unknown) notes.push(`${row.runs_unknown} unmeasured`);
  if (row.runs_partial) notes.push(`${row.runs_partial} partial`);
  if (row.runs_cost_unknown) notes.push(`${row.runs_cost_unknown} without USD cost`);
  if (row.runs_tokens_incomplete) notes.push(`${row.runs_tokens_incomplete} with incomplete tokens`);
  if (unattributed) notes.push(`${unattributed} unattributed (scope unknown)`);
  return notes.join(" · ");
}

export function spendRows(report) {
  return (report?.rows || []).map(row => {
    const tokens = row.tokens || {};
    const tokenTotal = [tokens.input, tokens.output, tokens.cache_read, tokens.cache_write]
      .reduce((sum, count) => sum + (count || 0), 0);
    return { ...row,
      name: row.label ? `${row.label} (${row.key})` : row.key,
      cost: row.runs > 0 && row.runs_unknown + row.runs_cost_unknown >= row.runs
        ? "unknown" : `${usd(row.cost_usd)} known`,
      tokenText: row.runs > 0 && row.runs_unknown >= row.runs
        ? "unknown" : `${tokenTotal.toLocaleString("en-US")} known`,
      uncertainty: uncertainty(row),
    };
  });
}

export function budgetCard(card) {
  const a = card.assessment;
  return { ...card,
    status: { within: "Within limit", over: "Over limit", unknown: "Unknown", unconfigured: "No limit" }[a.state] || "Unknown",
    limit: card.monthly_usd == null ? "not authored" : usd(card.monthly_usd),
    remaining: usd(a.remaining_usd),
    projected: usd(a.projected_month_usd),
    usedPercent: typeof a.used_fraction === "number" && Number.isFinite(a.used_fraction)
      ? Math.max(0, Math.min(100, a.used_fraction * 100)) : null,
    ancestorNote: card.relation === "ancestor" ? "Parent cap includes spending outside the selected subtree." : "",
  };
}

/// Observed daily burn-down uses frozen run usage assigned to its UTC start
/// day. Never draw an under-budget line if the amount remaining is unknown.
export function burnDown(card, month, width = 280, height = 80) {
  if (card.assessment.remaining_usd == null || !(card.monthly_usd > 0)) return null;
  const from = Date.parse(month.from), until = Date.parse(month.until), now = Date.parse(month.as_of);
  if (![from, until, now].every(Number.isFinite) || until <= from) return null;
  const x = time => Math.max(0, Math.min(width, (time - from) / (until - from) * width));
  const max = Math.max(card.monthly_usd, card.spent.cost_usd, 1);
  const y = amount => height - Math.max(0, Math.min(height, amount / max * height));
  const points = [`0,${y(card.monthly_usd).toFixed(2)}`];
  let spent = 0;
  for (const day of card.daily || []) {
    const time = Math.min(Date.parse(`${day.day}T00:00:00Z`) + 86400000, now);
    if (!Number.isFinite(time)) continue;
    spent += day.spent.cost_usd;
    points.push(`${x(time).toFixed(2)},${y(card.monthly_usd - spent).toFixed(2)}`);
  }
  points.push(`${x(now).toFixed(2)},${y(card.assessment.remaining_usd).toFixed(2)}`);
  return { points: points.join(" "), planned: `0,${y(card.monthly_usd).toFixed(2)} ${width},${height}`, width, height };
}

export function budgetEvent(ev) {
  return ["task_created", "task_updated", "task_deleted", "run_created", "run_updated"].includes(ev.type);
}
