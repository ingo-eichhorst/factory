//! What a run used, and what a task's runs used together (#117) -- pure
//! shaping for the task modal's usage block. A count the runtime could not
//! observe is `?`, and a run it could not measure says why; nothing here
//! ever turns an unknown into a zero.

/// Dollars to the cent; a non-zero amount under a cent says so rather than
/// reading as free.
export function fmtUsd(v) {
  if (v === null || v === undefined || !Number.isFinite(v)) return "?";
  if (v > 0 && v < 0.005) return "<$0.01";
  return `$${v.toFixed(2)}`;
}

export function fmtTokens(n) {
  if (n === null || n === undefined || !Number.isFinite(n)) return "?";
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

/// Every token type summed, or `null` if any one is unknown -- the same
/// rule as `TokenCounts::total` on the server.
export function totalTokens(tokens) {
  const t = tokens || {};
  const parts = [t.input, t.output, t.cache_read, t.cache_write];
  if (parts.some((p) => p === null || p === undefined)) return null;
  return parts.reduce((a, b) => a + b, 0);
}

/// A run's `usage` as lines for the modal: `{ tone, headline, lines, notes }`.
/// `tone` is `none` (no usage on record -- a run from before #117, or one
/// still coming up), `unknown` or `known`.
export function runUsageView(usage) {
  if (!usage) {
    return { tone: "none", headline: "no usage recorded for this run", lines: [], notes: [] };
  }
  if (usage.state !== "known") {
    return {
      tone: "unknown",
      headline: `unknown — ${usage.reason || "no reason recorded"}`,
      lines: [],
      notes: [],
    };
  }
  const t = usage.tokens || {};
  const atLeast = usage.partial ? "at least " : "";
  const cost = usage.cost_usd === null || usage.cost_usd === undefined ? "cost unknown" : fmtUsd(usage.cost_usd);
  const lines = [
    `${fmtTokens(t.input)} in · ${fmtTokens(t.output)} out · ${fmtTokens(t.cache_read)} cache read · ${fmtTokens(t.cache_write)} cache write`,
  ];
  if (usage.pricing_sources && usage.pricing_sources.length) lines.push(`priced by ${usage.pricing_sources.join(", ")}`);
  if (usage.models && usage.models.length) {
    lines.push(`${usage.models.join(", ")} · ${usage.sessions} harness session${usage.sessions === 1 ? "" : "s"}`);
  }
  if (usage.as_of) lines.push(`as of ${usage.as_of_point ? usage.as_of_point.replace("_", " ") : "the last reading"}`);
  return {
    tone: "known",
    headline: `${atLeast}${fmtTokens(totalTokens(t))} tokens · ${cost}`,
    lines,
    notes: usage.notes || [],
  };
}

/// One line for a task's sum over its runs (`GET /api/tasks/{id}/usage`'s
/// `total`), saying how many runs are not fully in it. `null` with no runs.
export function taskUsageLine(total) {
  if (!total || !total.runs) return null;
  const runs = `${total.runs} run${total.runs === 1 ? "" : "s"}`;
  const missing = [];
  if (total.runs_unknown) missing.push(`${total.runs_unknown} unmeasured`);
  if (total.runs_cost_unknown) missing.push(`${total.runs_cost_unknown} without a cost`);
  if (total.runs_partial) missing.push(`${total.runs_partial} partial`);
  const tokens = total.tokens || {};
  const sum = (tokens.input || 0) + (tokens.output || 0) + (tokens.cache_read || 0) + (tokens.cache_write || 0);
  if (total.runs_unknown === total.runs) return `All ${runs}: usage unknown`;
  return `All ${runs}: ${fmtTokens(sum)} tokens · ${fmtUsd(total.cost_usd)}${missing.length ? ` (not in the sum: ${missing.join(", ")})` : ""}`;
}
