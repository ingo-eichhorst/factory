//! Pure shaping logic for the L6 Direction Scenarios tab (`#100`) -- turns a
//! `ScenariosReport`/`ScenarioResult`/`ScenarioWhatIfResult` (see
//! `factory_core::scenario` and `Payload::Scenarios`/`ScenarioWhatIf` in
//! `protocol.rs`) into rows, chart geometry and request bodies `scenarios.js`
//! draws or sends. Nothing here touches `document` -- the same boundary
//! `policy-model.js`/`bench-model.js` keep, so a Node test needs no DOM at
//! all.
//!
//! Three vocabularies are fixed and compiled into `factory-core` but never
//! sent over the wire, the same way `policy-model.js`'s `KIND_LABELS`/
//! `REMEDIATION` mirror `factory_core::policy`'s own closed vocabularies:
//! `DRIVER_DEFS` mirrors `scenario::driver_defs()` (titles, units, which
//! driver is a bare assumption), `FIXED_METRIC_TITLES`/`metricTitle` covers
//! the handful of metric ids a scenario's signposts or drivers can name, and
//! the slider ranges in `driverRange` are this file's own judgement call,
//! not read off the server. A driver panel therefore fetches nothing beyond
//! `GET /api/scenarios` itself; only a metric's `title`/`unit`/`better` for
//! an id `baseline.metrics` actually names comes from a small
//! `GET /api/metrics?ids=…` call the view makes once per load (see
//! `metricTitle`'s `defs` parameter).

import { routeHref } from "./scopes.js";

// ------------------------------------------------------------------ drivers

/// The v1 built-in driver set (`factory_core::scenario::driver_defs()`),
/// mirrored here because it is fixed vocabulary compiled into the daemon,
/// never sent on the wire -- `ScenarioDrivers` only ever carries values keyed
/// by driver id. `better` is this file's own judgement (never asserted by
/// the server): higher effective throughput, first-pass yield and capacity
/// are wins; higher scrap, rework and cost are not.
export const DRIVER_DEFS = [
  { id: "throughput_week", title: "Throughput per week", description: "Finished runs in the trailing 7 days.", unit: "per_week", assumption: false, metric: "throughput_week", better: "higher" },
  { id: "first_pass_yield", title: "First-pass yield", description: "Finished runs that ended done without being rework, over finished, trailing 28 days.", unit: "ratio", assumption: false, metric: "first_pass_yield", better: "higher" },
  { id: "scrap_rate", title: "Scrap rate", description: "scrapped/finished, trailing 28 days.", unit: "ratio", assumption: false, metric: "scrap_rate", better: "lower" },
  { id: "rework_rate", title: "Rework rate", description: "Re-attempts of work that did not succeed, over finished, trailing 28 days.", unit: "ratio", assumption: false, metric: "rework_rate", better: "lower" },
  { id: "capacity_factor", title: "Capacity factor", description: "A multiplier on effective throughput with no data source -- a person's own what-if.", unit: "multiplier", assumption: true, metric: null, better: "higher" },
  // `unavailable: true` is fixed vocabulary, the same as `assumption` --
  // never derived from whether `GET /api/metrics` happened to answer this
  // load, so a slider for either of these is disabled even when that second
  // fetch fails outright, not just when it succeeds and says so.
  { id: "unit_cost", title: "Unit cost", description: "Cost per finished unit -- named by the registry but not yet computable (design §12.6).", unit: "ratio", assumption: false, unavailable: true, metric: "unit_cost", better: "lower" },
  { id: "tokens_per_run", title: "Tokens per run", description: "Tokens spent per run -- same §12.6 unavailability as unit cost.", unit: "count", assumption: false, unavailable: true, metric: "tokens_per_run", better: "lower" },
];

const UNAVAILABLE_REASON_FALLBACK = "a Run records no model, tokens or cost yet (design §12.6)";

/// The reason a `def.unavailable` driver is disabled -- preferring
/// `baseline.metrics`' own `reason` (on the wire on every `GET /api/scenarios`
/// answer, regardless of whether the second `GET /api/metrics?ids=…` fetch a
/// driver panel also makes ever succeeds), then the registry def that second
/// fetch supplies, then a fixed fallback in this file's own words -- a
/// disabled slider always says why, never a blank reason from a fetch that
/// happened to fail.
export function driverUnavailableReason(def, baselineMetrics, metricDefs) {
  const fromBaseline = (baselineMetrics || []).find((m) => m.id === def.metric);
  if (fromBaseline && fromBaseline.reason) return fromBaseline.reason;
  const fromRegistry = metricDefs && metricDefs[def.metric];
  if (fromRegistry && fromRegistry.unavailable_reason) return fromRegistry.unavailable_reason;
  return UNAVAILABLE_REASON_FALLBACK;
}

export function driverDef(id) {
  return DRIVER_DEFS.find((d) => d.id === id) || null;
}

/// Sensible slider bounds per driver -- ratios 0..1, the capacity multiplier
/// 0..2, throughput scaled to whatever the baseline is actually doing so a
/// company running at 40/week is not stuck sliding between 0 and 10.
export function driverRange(id, baselineValue) {
  switch (id) {
    case "throughput_week":
      return { min: 0, max: Math.max(10, (baselineValue || 0) * 2), step: 0.5 };
    case "capacity_factor":
      return { min: 0, max: 2, step: 0.05 };
    default:
      return { min: 0, max: 1, step: 0.01 };
  }
}

// ------------------------------------------------------------------ metrics

const FIXED_METRIC_TITLES = {
  throughput_week: "Throughput per week",
  first_pass_yield: "First-pass yield",
  scrap_rate: "Scrap rate",
  unit_cost: "Unit cost",
  tokens_per_run: "Tokens per run",
};

/// A metric id's display title. `defs` is the `{id: MetricDefView}` map the
/// view builds from `GET /api/metrics?ids=…` for exactly the ids
/// `baseline.metrics` names (see the header comment); this still falls back
/// to a family-prefix guess when that fetch failed or omitted an id, so the
/// page never shows a blank title.
export function metricTitle(id, defs) {
  if (defs && defs[id] && defs[id].title) return defs[id].title;
  if (FIXED_METRIC_TITLES[id]) return FIXED_METRIC_TITLES[id];
  const [family, ...rest] = id.split(".");
  const param = rest.join(".");
  switch (family) {
    case "compliance":
      return `Compliance share (${param})`;
    case "open_controls":
      return `Open controls (${param})`;
    case "bench":
      return `Bench resolve rate (${rest[rest.length - 1] || param})`;
    case "goal_tasks_done":
      return `Goal tasks done (${param})`;
    default:
      return id;
  }
}

// -------------------------------------------------------------- signposts

const SIGNPOST_LABELS = { quiet: "quiet", triggered: "triggered", not_yet_active: "not yet active", no_data: "no data" };

export function signpostStateInfo(state) {
  return { cls: `scn-sp-${state}`, label: SIGNPOST_LABELS[state] || state };
}

/// "below 8", "above 0.9", or "outside 0.4–0.8" when both are set (a
/// two-sided band a signpost may watch, per `evaluate_signposts`' own doc
/// comment) -- the reason string itself already says which side triggered,
/// this is only the threshold as authored.
export function signpostThreshold(sp) {
  const hasBelow = sp.below !== null && sp.below !== undefined;
  const hasAbove = sp.above !== null && sp.above !== undefined;
  if (hasBelow && hasAbove) return `outside ${sp.above}–${sp.below}`;
  if (hasBelow) return `below ${sp.below}`;
  if (hasAbove) return `above ${sp.above}`;
  return "no threshold";
}

/// One scenario's signposts, its own `Signpost` definitions (thresholds)
/// zipped by index against its `SignpostStatus` results -- the same
/// index-pairing `scenario_detail_text` uses in `factory-cli/src/main.rs`
/// (`s.signposts.iter().zip(&s.scenario.signposts)`), since neither wire
/// shape carries a shared key and `evaluate_signposts` preserves order.
export function signpostRows(result) {
  const defs = result.scenario.signposts || [];
  const statuses = result.signposts || [];
  const n = Math.min(defs.length, statuses.length);
  const rows = [];
  for (let i = 0; i < n; i++) {
    const def = defs[i];
    const status = statuses[i];
    rows.push({
      scenario: result.scenario.name,
      scenarioTitle: result.scenario.title,
      metric: status.metric,
      state: status.state,
      reason: status.reason,
      threshold: signpostThreshold(def),
      from: def.from || null,
      ...signpostStateInfo(status.state),
    });
  }
  return rows;
}

/// Every signpost across every scenario, flattened for the strip at the top
/// of the page -- unlike `ScenariosReport.triggered`, this carries every
/// state (quiet, not-yet-active, no-data too), which is what a strip of
/// lights needs to show.
export function signpostStrip(report) {
  const rows = [];
  for (const s of report.scenarios || []) rows.push(...signpostRows(s));
  return rows;
}

// -------------------------------------------------------------- fan charts

/// `THROUGHPUT_HISTORY_WEEKS` in `factory_core::scenario` -- 26
/// non-overlapping weekly sums, chosen to match `Horizon::default()`'s own
/// 26 weeks (README, "Throughput history"). Fixed, compiled-in vocabulary,
/// mirrored here the same way `DRIVER_DEFS` mirrors `driver_defs()`, so the
/// no-signal empty state can name the real window without a value the wire
/// never sends.
export const THROUGHPUT_HISTORY_WEEKS = 26;

/// A fan chart's geometry for one `Forecast` -- p10/p90 band polygon, the
/// p50 polyline, and (when the caller passes a completion week) a marker on
/// each line at the week it lands on. Deliberately reads the marker's `y`
/// straight off the line it sits on rather than requiring a `backlog`
/// value: `Forecast` itself carries no backlog (`forecast_completion` takes
/// it as a parameter, never stores it), so this works for the baseline card
/// (whose real backlog -- "every non-terminal task right now" -- never
/// reaches the wire at all) exactly the way it works for a scenario's own
/// card. `backlog`, when given, only draws the dashed reference line a
/// scenario's own `ScenarioBacklog.total` places on its band.
///
/// Two states are worth drawing as an explicit empty message instead of a
/// flat, empty-looking chart (`scenarios.js`'s own job, this file only says
/// which applies): `clearedAlready` -- `completion_week.p50 === 0`, which
/// `forecast_completion` only ever produces when `backlog <= 0.0` (every
/// sample completes at week 0 identically; a real backlog can only clear at
/// week 1 or later) -- and `noSignal` -- every week's own p90 is exactly
/// zero, meaning the throughput history sampled from had nothing in it.
export function fanGeometry(forecast, opts = {}) {
  const width = opts.width ?? 600;
  const height = opts.height ?? 160;
  const backlog = opts.backlog ?? null;
  if (!forecast || !forecast.per_week || forecast.per_week.length === 0) {
    return { empty: true, reason: (forecast && forecast.reason) || "no forecast to draw" };
  }
  const n = forecast.per_week.length;
  const maxY = Math.max(backlog || 0, ...forecast.per_week.map((p) => p.p90), 1e-9);
  const xStep = width / n;
  const x = (weekIndex) => weekIndex * xStep;
  const y = (v) => height - (Math.min(Math.max(v, 0), maxY) / maxY) * height;

  const p10Line = [[0, height]].concat(forecast.per_week.map((p, i) => [x(i + 1), y(p.p10)]));
  const p90Line = [[0, height]].concat(forecast.per_week.map((p, i) => [x(i + 1), y(p.p90)]));
  const medianLine = [[0, height]].concat(forecast.per_week.map((p, i) => [x(i + 1), y(p.p50)]));
  const fmt = (pts) => pts.map(([px, py]) => `${px.toFixed(1)},${py.toFixed(1)}`).join(" ");

  const markerFor = (week, line) => {
    if (week === null || week === undefined || week < 1 || week > n) return null;
    const [mx, my] = line[week];
    return { week, x: mx, y: my };
  };

  // A handful of x ticks -- "now" (week 0), a midpoint, and the horizon's
  // last week -- never more: the issue's own "keep it light" for this
  // chart. `midWeek` is skipped when it would land on "now" or the last
  // week itself (a one- or two-week horizon), so two ticks never collide.
  const midWeek = Math.round(n / 2);
  const xTicks = [{ x: 0, label: "now" }];
  if (midWeek > 0 && midWeek < n) xTicks.push({ x: x(midWeek), week: midWeek, label: `week ${midWeek}` });
  xTicks.push({ x: width, week: n, label: `week ${n}` });

  return {
    empty: false,
    noSignal: forecast.per_week.every((p) => p.p90 === 0),
    clearedAlready: forecast.completion_week.p50 === 0,
    width,
    height,
    weeks: n,
    yMax: maxY,
    xTicks,
    bandPoints: fmt(p10Line.concat([...p90Line].reverse())),
    medianPoints: fmt(medianLine),
    p50Marker: markerFor(forecast.completion_week.p50, medianLine),
    p90Marker: markerFor(forecast.completion_week.p90, p90Line),
    backlogY: backlog !== null && backlog !== undefined ? y(backlog) : null,
  };
}

export function formatWeek(w) {
  return w === null || w === undefined ? "never within the horizon" : `week ${w}`;
}

/// The card's one-line completion summary -- "nothing in the backlog to
/// clear" when there is nothing to clear, `p50 …  p90 …` otherwise. Prefers
/// a known `backlogTotal` (a scenario's own `ScenarioBacklog.total`, always
/// on the wire) when the caller has one; falls back to the same
/// `completion_week.p50 === 0` proof `fanGeometry`'s own `clearedAlready`
/// uses for the baseline card, which carries no backlog of its own at all.
export function completionSummary(fc, backlogTotal) {
  const cleared = backlogTotal !== null && backlogTotal !== undefined ? backlogTotal <= 0 : fc.p50 === 0;
  if (cleared) return "nothing in the backlog to clear";
  return `p50 ${formatWeek(fc.p50)} · p90 ${formatWeek(fc.p90)}`;
}

// -------------------------------------------------------------- delta table

/// `before`/`after` classified into a colour direction and a rough
/// magnitude bucket, never a raw percentage -- the delta table colours by
/// direction and size, not by an exact number a person would have to read
/// twice. `null`/`undefined`/non-finite on either side is `unknown`: no
/// comparison is honest to draw, the same restraint a `None` probability
/// takes rather than a fabricated number.
export function deltaTone(before, after, better) {
  if (before === null || before === undefined || after === null || after === undefined || !Number.isFinite(before) || !Number.isFinite(after)) {
    return { tone: "unknown", mag: 0 };
  }
  const diff = after - before;
  if (diff === 0) return { tone: "flat", mag: 0 };
  const worse = better === "lower" ? diff > 0 : diff < 0;
  // Relative to `before` -- the ordinary reading of "a 30% change" -- falling
  // back to `after` only when `before` itself is zero (nothing to be a
  // percentage of), and to a tiny epsilon only when both are zero (already
  // handled above by the `diff === 0` return, so this never actually divides
  // by it; it only keeps the expression finite).
  const scale = Math.abs(before) || Math.abs(after) || 1e-9;
  const rel = Math.abs(diff) / scale;
  const mag = rel < 0.05 ? 1 : rel < 0.2 ? 2 : 3;
  return { tone: worse ? "bad" : "good", mag };
}

function openStale(rollup) {
  return rollup ? rollup.counts.open + rollup.counts.stale : null;
}

/// The delta table's rows: driver values (baseline vs. each scenario's own
/// `overridden`), the one v1 outcome (`effective_throughput`, before/after
/// *within* each scenario -- `outcomes_before` already reads the shared
/// baseline drivers, so there is no separate "baseline outcome" to compute),
/// the forecast's completion week at p50/p90 (baseline vs. scenario,
/// `exact: false` -- these are bands collapsed to one number for the table;
/// the fan chart is where the band itself lives), and, per framework any
/// scenario's policy delta touches, the open+stale count before/after
/// (`exact: true`, from `policy_subtree.per_framework`, the same rollup the
/// Policy tab itself shows).
export function deltaTable(report) {
  const scenarios = report.scenarios || [];
  const rows = [];

  for (const def of DRIVER_DEFS) {
    const before = report.baseline.drivers[def.id];
    const cells = scenarios.map((s) => {
      const after = s.drivers.overridden[def.id];
      return { scenario: s.scenario.name, before, after, ...deltaTone(before, after, def.better) };
    });
    if (before === undefined && cells.every((c) => c.after === undefined)) continue;
    rows.push({ key: `driver:${def.id}`, label: def.title, unit: def.unit, kind: "driver", exact: true, cells });
  }

  rows.push({
    key: "outcome:effective_throughput",
    label: "Effective throughput",
    unit: "per_week",
    kind: "outcome",
    exact: true,
    cells: scenarios.map((s) => {
      const before = s.drivers.outcomes_before.effective_throughput;
      const after = s.drivers.outcomes_after.effective_throughput;
      return { scenario: s.scenario.name, before, after, ...deltaTone(before, after, "higher") };
    }),
  });

  for (const p of ["p50", "p90"]) {
    rows.push({
      key: `forecast:${p}`,
      label: `Completion week (${p})`,
      unit: "week",
      kind: "forecast",
      exact: false,
      cells: scenarios.map((s) => {
        const before = report.baseline.forecast.completion_week[p];
        const after = s.forecast.completion_week[p];
        return { scenario: s.scenario.name, before, after, ...deltaTone(before, after, "lower") };
      }),
    });
  }

  const frameworks = new Set();
  for (const s of scenarios) for (const fd of s.policy_subtree.per_framework) frameworks.add(fd.framework);
  for (const fw of [...frameworks].sort()) {
    rows.push({
      key: `policy:${fw}`,
      label: `${fw}: open + stale controls`,
      unit: "count",
      kind: "policy",
      exact: true,
      cells: scenarios.map((s) => {
        const fd = s.policy_subtree.per_framework.find((x) => x.framework === fw);
        const before = fd ? openStale(fd.before) : null;
        const after = fd ? openStale(fd.after) : null;
        return { scenario: s.scenario.name, before, after, ...deltaTone(before, after, "lower") };
      }),
    });
  }

  return { scenarios: scenarios.map((s) => s.scenario.name), rows };
}

/// "+5 open controls in ai-act, 2 already covered" -- one scenario's own
/// compliance delta, summed over its subtree-wide `policy_subtree` rather
/// than per scope, for the card's one-line summary.
export function complianceSummary(result) {
  const d = result.policy_subtree;
  if (!d || (d.newly_open.length === 0 && d.newly_stale.length === 0 && d.newly_applicable_but_covered.length === 0)) {
    return "No change to any control's status.";
  }
  const parts = [];
  if (d.newly_open.length) parts.push(`${d.newly_open.length} newly open`);
  if (d.newly_stale.length) parts.push(`${d.newly_stale.length} newly stale`);
  let text = parts.length ? parts.join(", ") + " control" + (d.newly_open.length + d.newly_stale.length === 1 ? "" : "s") : "no newly open or stale controls";
  if (d.newly_applicable_but_covered.length) {
    text += `, ${d.newly_applicable_but_covered.length} already covered`;
  }
  return text;
}

// ---------------------------------------------------------------- tornado

/// Horizontal-bar layout for a tornado chart: every bar on one shared scale
/// (the min of every `low_outcome` to the max of every `high_outcome`), so
/// spans are visually comparable across drivers. Re-sorts by span descending
/// (ties by driver ascending) -- the same order `scenario::tornado` itself
/// produces server-side -- so a caller handing this an unsorted list (a
/// what-if response layered oddly, a hand-built test fixture) still draws
/// correctly. A driver that does not move the outcome at all (`span: 0`,
/// e.g. `rework_rate` in v1's `effective_throughput` formula) is kept, not
/// dropped -- `hairline: true` says so, so the view can still draw a thin
/// marker rather than silently losing the row.
export function tornadoLayout(bars, opts = {}) {
  const width = opts.width ?? 400;
  const sorted = [...(bars || [])].sort((a, b) => b.span - a.span || a.driver.localeCompare(b.driver));
  if (sorted.length === 0) return { bars: [], min: 0, max: 0, width };
  const min = Math.min(...sorted.map((b) => b.low_outcome));
  const max = Math.max(...sorted.map((b) => b.high_outcome));
  const span = max - min || 1;
  const scaleX = (v) => ((v - min) / span) * width;
  return {
    width,
    min,
    max,
    bars: sorted.map((b) => {
      const x = scaleX(b.low_outcome);
      const w = scaleX(b.high_outcome) - x;
      return { driver: b.driver, low: b.low_outcome, high: b.high_outcome, span: b.span, x, width: w, hairline: w === 0 };
    }),
  };
}

// ---------------------------------------------------------------- what-if

/// `=<value>` -- `Override::Set`, the one variant that needs no baseline to
/// stand alone (`apply_overrides`' own doc comment): a slider's value is
/// already absolute, so sending it as a delta or a multiplier would have to
/// know the baseline the server itself resolves, and would silently do
/// nothing for a driver the baseline carries no value for at all
/// (`first_pass_yield`/`scrap_rate` in a fresh instance with no finished
/// runs -- see `apply_overrides`). Rounded to four decimal places so a
/// float slider never sends `0.30000000000000004`.
export function sliderToOverride(value) {
  const rounded = Math.round(value * 10000) / 10000;
  return `=${rounded}`;
}

/// The body `POST /api/scenarios/whatif` expects, from the drivers a person
/// actually moved (`moved`, `{id: number}`) -- never every driver on every
/// call, so an untouched slider does not silently reassert its own starting
/// value as an override on top of whichever scenario is selected.
export function whatifBody(scenarioName, moved) {
  const drivers = {};
  for (const [id, v] of Object.entries(moved || {})) drivers[id] = sliderToOverride(v);
  const body = { drivers };
  if (scenarioName) body.scenario = scenarioName;
  return body;
}

// ---------------------------------------------------------------- promote

export function promoteBody(scenario, scope, agent) {
  const body = { scenario, scope };
  if (agent) body.agent = agent;
  return body;
}

// --------------------------------------------------------- policy what-if

function frameworkOf(ref) {
  return ref.split("/")[0];
}

/// scope × framework counts of newly-open / newly-stale / already-covered
/// controls, aggregated from `ScenarioResult.policy` -- the per-scope
/// deltas a report already carries, bucketed by each `ControlRef`'s own
/// framework (`policy_subtree`'s own `per_framework` aggregates rollups, not
/// control lists, so this reads the per-scope `delta` arrays instead).
export function matrixCells(result) {
  const scopes = (result.policy || []).map((p) => p.scope);
  const frameworks = new Set();
  const cells = {};
  const bucket = (scope, list, key) => {
    for (const ref of list) {
      const fw = frameworkOf(ref);
      frameworks.add(fw);
      cells[scope][fw] = cells[scope][fw] || { newly_open: [], newly_stale: [], covered: [] };
      cells[scope][fw][key].push(ref);
    }
  };
  for (const p of result.policy || []) {
    cells[p.scope] = cells[p.scope] || {};
    bucket(p.scope, p.delta.newly_open, "newly_open");
    bucket(p.scope, p.delta.newly_stale, "newly_stale");
    bucket(p.scope, p.delta.newly_applicable_but_covered, "covered");
  }
  return { scopes, frameworks: [...frameworks].sort(), cells };
}

/// Why one control shows up in the matrix -- derived from the scenario's own
/// overlay, since `PolicyDelta` carries bare `ControlRef`s with no reason of
/// its own (unlike a `ControlStatus`'s reasons, which explain *evidence*,
/// not *why this control exists under this scenario at all*) and
/// `GET /api/policy/controls/…` would answer for the real chain, not this
/// scenario's overlay. Framework introduced by `add_frameworks` takes
/// priority over a tighten on the same control -- the two are not mutually
/// exclusive to check, but a whole framework appearing is the more useful
/// thing to say first.
export function controlReason(scenario, ref, bucketKey) {
  if (bucketKey === "covered") return "already satisfied or attested once this framework applies here";
  const overlay = scenario.policy;
  if (!overlay) return "newly open or stale under this scenario's overlay";
  const fw = frameworkOf(ref);
  if ((overlay.add_frameworks || []).includes(fw)) return `framework ${fw} added by this scenario`;
  const tighten = (overlay.tighten || {})[ref];
  if (tighten && tighten.max_age) return `tightened to max_age ${tighten.max_age}`;
  return "newly open or stale under this scenario's overlay";
}

// --------------------------------------------------------------- workshop

/// Group every `narrative`-kind scenario by its own `quadrant` name into a
/// 2x2 board. `quadrant` is free text ("the author's own words", per
/// `Narrative`'s doc comment), so there is no canonical top-left/top-right
/// mapping to place a name at -- this groups by the quadrant string itself,
/// in first-encounter order, which is stable and lets several scenarios
/// share one quadrant honestly rather than guessing a position from a name
/// this file has never seen before. `axes` is read off the first narrative
/// scenario that names any, on the working assumption a workshop's own
/// scenarios share one 2x2 -- a mismatched second `axes` is not merged or
/// flagged, just left unread, since this is qualitative content no
/// computation here scores or validates (the module doc comment's own
/// "Layering" rule).
export function narrativeBoard(scenarios) {
  const narrativeScenarios = (scenarios || []).filter((s) => s.scenario.narrative);
  let axes = null;
  const order = [];
  const byQuadrant = new Map();
  for (const s of narrativeScenarios) {
    const n = s.scenario.narrative;
    if (!axes && n.axes && n.axes.length) axes = n.axes;
    const name = n.quadrant || "(no quadrant named)";
    if (!byQuadrant.has(name)) {
      byQuadrant.set(name, []);
      order.push(name);
    }
    byQuadrant.get(name).push(s);
  }
  return {
    axes,
    quadrants: order.map((name) => ({ quadrant: name, scenarios: byQuadrant.get(name) })),
    pestle: narrativeScenarios.filter((s) => (s.scenario.narrative.drivers || []).length > 0).map((s) => ({ scenario: s.scenario.name, drivers: s.scenario.narrative.drivers })),
    premortem: narrativeScenarios.filter((s) => (s.scenario.narrative.premortem || []).length > 0).map((s) => ({ scenario: s.scenario.name, points: s.scenario.narrative.premortem })),
  };
}

// -------------------------------------------------------------------- misc

/// `KrRef` serialises as `"objective/kr"` (confirmed against a running
/// daemon's `GET /api/scenarios` -- `ControlRef`'s own sibling type follows
/// the same `try_from`/`into` `String` pattern); this still accepts the
/// `{objective, kr}` object shape defensively, the same normalising trick
/// `policy-model.js`'s `statusOf` plays for a status that can arrive
/// flattened or nested depending which endpoint sent it.
export function krLabel(kr) {
  if (typeof kr === "string") return kr;
  if (kr && kr.objective && kr.kr) return `${kr.objective}/${kr.kr}`;
  return String(kr);
}

/// "42%", or "not modelled — <reason>" for the `None` a `Ratio` key result
/// (or an already-reached/already-passed target) always carries instead of
/// a fabricated number -- the issue's own phrasing for `GoalProbability`.
export function goalProbabilityLabel(gp) {
  if (gp.probability === null || gp.probability === undefined) {
    return `not modelled — ${gp.reason || "no reason given"}`;
  }
  return `${Math.round(gp.probability * 100)}%`;
}

/// A link to the Policy tab, scoped -- the matrix modal's and a card's own
/// "jump to Policy" link. Never a specific control: `policy.js`'s own
/// control-detail modal is ephemeral UI state with no route of its own (see
/// `policy-model.js`'s header comment), so there is nowhere more specific
/// than the tab itself to point at.
export function policyTabHref(scope) {
  return routeHref(scope, "policy");
}
