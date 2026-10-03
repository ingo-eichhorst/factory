//! Pure shaping logic for the L6 Direction, Goals tab (GitHub issue `#99`,
//! slice 3): everything that turns a `GoalsReport` (see `factory_core::goals`
//! and `Payload::Goals` in `protocol.rs`) and a `Metrics` answer
//! (`Payload::Metrics`) into the numbers and rows the strategy map, roadmap,
//! orbit and table draw.
//!
//! Nothing here touches `document`, `window`, or `localStorage` -- `goals.js`
//! is the only module that draws anything or measures anything on screen, so
//! this one is safe to import in a Node test with no DOM at all, the same
//! boundary `policy-model.js`/`bench-model.js` already keep. It does import
//! `routeHref` from `scopes.js`, which is itself DOM-free.
//!
//! ## Two wire shapes worth naming precisely
//!
//! `GoalsReport` (the `/api/goals` answer, unwrapped by `api()`) nests a
//! `CycleReport` under its own field also called `report` -- so
//! `goalsReport.report.objectives[i].key_results[j]` is a `KrResult`, not
//! `goalsReport.objectives`. `KrRef` and `MetricId` both serialize as a bare
//! string (`"objective/kr"`, `"compliance.cra"`), confirmed against a real
//! `GET /api/goals` and `POST /api/goals/checkins` on a throwaway daemon --
//! `report.checkins` is a plain `{ "objective/kr": [...] }` object, and
//! `KrResult.kr` and `KrResult.metric` are themselves already those strings,
//! never a nested `{objective, kr}` pair on the wire.
//!
//! ## Sparklines are honest, not invented
//!
//! `/api/goals` carries no time series at all. `/api/metrics` only computes
//! one for the three production-based metrics (`throughput_week`,
//! `first_pass_yield`, `scrap_rate` -- confirmed against `factory-daemon`'s
//! `metrics.rs`, whose own doc comment says so). So a manual key result's
//! sparkline comes from its own check-in history (`report.checkins`), a
//! production-metric key result's from `/api/metrics`' `series`, and every
//! other metric-backed key result (`compliance.*`, `bench.*`,
//! `goal_tasks_done.*`) gets none -- `dashboard.js`'s own sparkline is the
//! precedent for refusing to invent one ("a sparkline there would have to be
//! invented").

import { routeHref } from "./scopes.js";

// -------------------------------------------------------------- cycles

/// Which cycle to show when nothing was asked: the current one if there is
/// one, else the soonest future one (a company between cycles is looking
/// ahead), else the most recently ended past one -- never just "the first
/// in the list", which is whatever order the files happened to sort in.
/// ISO dates (`from`/`to`) sort correctly as plain strings.
export function defaultCycleId(cycles) {
  const list = cycles || [];
  if (!list.length) return null;
  const current = list.find((c) => c.status === "current");
  if (current) return current.id;
  const future = list.filter((c) => c.status === "future").sort((a, b) => a.from.localeCompare(b.from));
  if (future.length) return future[0].id;
  const past = list.filter((c) => c.status === "past").sort((a, b) => b.to.localeCompare(a.to));
  if (past.length) return past[0].id;
  return list[0].id;
}

/// The cycle switcher's own option label -- id, status and score in one
/// line, so a person can tell past/current/future and how it is doing apart
/// without opening it first. `score` reads `cycleSummary.score` verbatim --
/// see `cycleProgress`'s own header comment for why that, and not a
/// client-recomputed figure, is the one definition this page uses
/// everywhere a cycle's score is shown.
export function cycleOptionLabel(cycleSummary) {
  const score = cycleSummary.score === null || cycleSummary.score === undefined ? "unscored" : `${Math.round(cycleSummary.score * 100)}% score`;
  return `${cycleSummary.id} — ${cycleSummary.status} — ${score}`;
}

/// The cycle progress bar's two figures: how much of the calendar window has
/// elapsed, and the cycle's own score -- the mean of every *objective's*
/// own score (each already a mean of that objective's own key results),
/// never a flat mean across every key result directly. That is deliberate,
/// not an arbitrary choice: it is the exact formula the daemon's own
/// `mean_score` uses to fill `CycleSummary.score` (`GoalsReport.cycles`,
/// the cycle switcher's own numbers, via `cycleOptionLabel`) -- computing it
/// the same way here, from the same `objectives` array the switcher's
/// selected cycle already carries, guarantees the switcher and this bar
/// can never disagree, rather than hoping two independently-computed
/// aggregates happen to match. A flat key-result mean is a different,
/// equally defensible number, but it cannot be computed for the *other*
/// cycles the switcher lists (`GoalsReport.report` only ever evaluates the
/// one asked-about cycle), so it can never be the one consistent
/// definition across the whole switcher.
///
/// The bar draws `scorePct` as its fill and `elapsedPct` as a thin
/// "expected by now" marker: a committed key result is meant to close
/// linearly over the cycle (`KrResult::on_pace`'s own rule), so the marker
/// says where the fill would sit if every key result were exactly on pace.
/// `null` when there is no report to show (no current cycle, or the asked
/// cycle was refused).
export function cycleProgress(cycleReport) {
  if (!cycleReport) return null;
  const elapsedPct = pct(cycleReport.elapsed);
  const objectiveScores = (cycleReport.objectives || [])
    .map((o) => o.score)
    .filter((s) => s !== null && s !== undefined);
  const scorePct = objectiveScores.length ? pct(objectiveScores.reduce((a, b) => a + b, 0) / objectiveScores.length) : null;
  return { elapsedPct, scorePct };
}

/// How many of the cycle's own key results carry a score at all -- a plain
/// count, deliberately kept apart from `cycleProgress`'s own `scorePct`
/// (an aggregate of the *scored* ones): "48% score" and "3 of 5 KRs scored"
/// are two different facts, and folding the second into the first's own
/// wording is what read as inconsistent before this file settled on one
/// score definition.
export function krScoredCount(cycleReport) {
  let scored = 0;
  let total = 0;
  for (const o of (cycleReport && cycleReport.objectives) || []) {
    for (const kr of o.key_results || []) {
      total += 1;
      if (kr.score !== null && kr.score !== undefined) scored += 1;
    }
  }
  return { scored, total };
}

function pct(fraction) {
  return Math.round(Math.max(0, Math.min(1, fraction)) * 1000) / 10;
}

// ---------------------------------------------------------------- bands

/// `Band` (`green`/`yellow`/`red`) or `null` (unscored, drawn grey) -- one
/// word either way, matching the wire's own spelling.
export function bandLabel(band) {
  return band || "unscored";
}

/// The shared `.badge.s-*` convention `policy-model.js`'s `statusBadge` also
/// draws from, extended with the four bands here.
export function bandBadgeClass(band) {
  return `s-${bandLabel(band)}`;
}

// ----------------------------------------------------------------- rings

/// A key result's progress ring is drawn at this radius unless a caller
/// needs a different one -- 28, so a 6px stroke fits a ~64px card ring
/// (the design pass's own number) with a hair of padding; an objective's
/// own smaller aggregate ring (`goals.js`'s `objectiveRingSvg`) passes its
/// own radius instead.
export const KR_RING_RADIUS = 28;

/// `{radius, circumference, offset}` for an SVG `<circle>` progress ring:
/// `stroke-dasharray: circumference`, `stroke-dashoffset: offset`, drawn
/// full at `score: 1` and empty at `score: null` (never a manufactured
/// `0` reading as "scored zero" -- an empty ring and a red full one look
/// different on purpose).
export function ringGeometry(score, radius = KR_RING_RADIUS) {
  const r = Number(radius) || KR_RING_RADIUS;
  const circumference = 2 * Math.PI * r;
  const clamped = score === null || score === undefined ? 0 : Math.max(0, Math.min(1, score));
  return { radius: r, circumference, offset: circumference * (1 - clamped) };
}

/// `stroke-dasharray` for the ring's own track (not the progress arc): `null`
/// for a committed key result (a promise, drawn solid) and a short dash
/// pattern for an aspirational one (a stretch) -- the issue's own guardrail
/// that committed and aspirational must be "visibly distinct", not just a
/// colour a legend has to explain.
export function ringDashArray(kind, circumference) {
  if (kind !== "aspirational") return null;
  const unit = circumference / 40;
  return `${(unit * 0.6).toFixed(1)} ${(unit * 0.4).toFixed(1)}`;
}

// -------------------------------------------------------------- sparklines

/// A polyline's own `points` attribute for a small trend line, `null` when
/// there are fewer than two values to draw between -- the exact formula
/// `dashboard.js`'s own `sparkline()` uses, so the two draw identically
/// rather than two slightly different implementations of the same idea.
export function sparklinePointsAttr(values, w = 100, h = 26) {
  if (!values || values.length < 2) return null;
  const max = Math.max(...values);
  const min = Math.min(...values);
  const span = max - min || 1;
  const step = w / (values.length - 1);
  return values.map((v, i) => `${(i * step).toFixed(1)},${(h - ((v - min) / span) * h).toFixed(1)}`).join(" ");
}

/// The values behind a key result's own sparkline, oldest first -- from its
/// check-in history for a manual key result, from `/api/metrics`' series for
/// the three production metrics, or `null` (no history, never invented) for
/// anything else. `checkinsByRef` is `report.checkins` (keyed by the same
/// `"objective/kr"` string `kr.kr` already is); `seriesById` maps a metric id
/// to its `MetricSeries`.
export function krHistoryValues(kr, checkinsByRef, seriesById) {
  if (kr.manual) {
    const history = (checkinsByRef && checkinsByRef[kr.kr]) || [];
    return history.length >= 2 ? history.map((c) => c.value) : null;
  }
  const series = kr.metric && seriesById && seriesById[kr.metric];
  if (!series || !series.points || series.points.length < 2) return null;
  return series.points.map(([, v]) => v);
}

// -------------------------------------------------------------- metrics

/// The three metrics `factory-daemon` ever computes a series for -- see the
/// module doc comment. Always fetched, whether or not any key result reads
/// one this cycle, so the header's north-star/input tiles can carry a trend
/// too.
export const PRODUCTION_METRIC_IDS = ["throughput_week", "first_pass_yield", "scrap_rate"];

/// Every metric id worth a second `/api/metrics?ids=` fetch alongside
/// `/api/goals`: the north star, every input, and every key result's own
/// bound metric in the cycle actually being shown -- deduplicated, sorted
/// for a deterministic query string. `goalsReport` is the whole
/// `GoalsReport` (so `.report` is the nested `CycleReport`, see the module
/// doc comment).
export function metricIdsForReport(goalsReport) {
  const ids = new Set(PRODUCTION_METRIC_IDS);
  if (goalsReport.north_star) ids.add(goalsReport.north_star.metric);
  for (const i of goalsReport.inputs || []) ids.add(i.metric);
  if (goalsReport.report) {
    for (const o of goalsReport.report.objectives || []) {
      for (const kr of o.key_results || []) {
        if (kr.metric) ids.add(kr.metric);
      }
    }
  }
  return [...ids].sort();
}

/// `id -> MetricDefView` and `id -> MetricSeries`, from a `/api/metrics`
/// answer -- built once per fetch rather than `.find()`ing the arrays
/// repeatedly while drawing.
export function registryIndex(metricsAnswer) {
  const map = {};
  for (const d of (metricsAnswer && metricsAnswer.registry) || []) map[d.id] = d;
  return map;
}
export function seriesIndex(metricsAnswer) {
  const map = {};
  for (const s of (metricsAnswer && metricsAnswer.series) || []) map[s.id] = s;
  return map;
}

/// `{value, as_of, reason}` -> a plain number string per `Unit`
/// (`ratio`/`count`/`per_week`/`seconds`/`hours`/`days`/`usd`), or an em dash
/// for `null` -- display only, never used in a computation. `unit` is `null`
/// for a manual key result (no registry entry backs it) or when the registry
/// has not answered yet; either way this falls back to a trimmed plain
/// number rather than guessing a unit. `hours` is #150/#162's `agent_hours`/
/// `blocked_hours`: one decimal, the same precision the dashboard's own
/// agent-hours tiles show, never rounded to a whole hour. `days` is #154's
/// `backup_verified_age_days`: the same one-decimal precision as `hours`,
/// since a verification's age is read in whole-ish days, not fractions of
/// an hour.
export function formatUnitValue(value, unit) {
  if (value === null || value === undefined || !Number.isFinite(value)) return "—";
  switch (unit) {
    case "ratio":
      return `${(value * 100).toFixed(1)}%`;
    case "count":
      return Number.isInteger(value) ? String(value) : value.toFixed(1);
    case "per_week":
      return `${value.toFixed(1)}/wk`;
    case "seconds":
      return formatSeconds(value);
    case "hours":
      return `${value.toFixed(1)}h`;
    case "days":
      return `${value.toFixed(1)}d`;
    case "usd":
      return value > 0 && value < 0.005 ? "<$0.01" : `$${value.toFixed(2)}`;
    default:
      return String(Math.round(value * 100) / 100);
  }
}

function formatSeconds(value) {
  if (value < 60) return `${Math.round(value)}s`;
  if (value < 3600) return `${Math.round(value / 60)}m`;
  return `${(value / 3600).toFixed(1)}h`;
}

/// The honest line for a metric this build cannot compute -- `unit_cost`,
/// `tokens_per_run`, or any other value the daemon sent back as
/// `value: null` with a `reason` -- read verbatim from the wire rather than
/// shown as `0` (design's own "not measured yet" rule, the issue's own
/// guardrail for §12.6's cost metrics). `null` when `mv` has a real value.
export function metricUnavailableNote(mv) {
  if (!mv || mv.value !== null && mv.value !== undefined) return null;
  return mv.reason ? `not measured yet — ${mv.reason}` : "not measured yet";
}

// ------------------------------------------------------------ source links

/// Where a key result's own evidence lives, by its bound metric's own
/// prefix -- the issue's own routing table. `null` for a manual key result
/// (there is no source but the check-in form itself) and for a metric this
/// build does not know how to route (`unit_cost`, `tokens_per_run`, or a
/// future family) -- a missing link is honest; a guessed one is not.
/// `goal_tasks_done` has no per-label filter in the Tasks view today, so it
/// links to the Tasks tab whole, same as the issue's own fallback.
export function krSourceLink(kr, scope) {
  if (kr.manual || !kr.metric) return null;
  const metric = kr.metric;
  if (metric.startsWith("compliance.") || metric.startsWith("open_controls.")) {
    return { label: "Policy", href: routeHref(scope, "policy") };
  }
  if (metric.startsWith("goal_tasks_done")) {
    return { label: "Tasks", href: routeHref(scope, "tasks") };
  }
  if (metric.startsWith("bench.")) {
    return { label: "Benchmarks", href: routeHref(scope, "benchmarks") };
  }
  if (PRODUCTION_METRIC_IDS.includes(metric)) {
    return { label: "Dashboard", href: routeHref(scope, "dashboard") };
  }
  return null;
}

// ------------------------------------------------------------------ table

/// One row per key result in the asked cycle -- the accessible equivalent
/// the issue asks for, and the flat shape the strategy map's own cards
/// build from too.
export function tableRows(goalsReport) {
  const rows = [];
  const cycleReport = goalsReport && goalsReport.report;
  if (!cycleReport) return rows;
  for (const o of cycleReport.objectives || []) {
    for (const kr of o.key_results || []) {
      rows.push({
        objective: o.objective,
        objectiveTitle: o.title,
        kr: kr.kr,
        title: kr.title,
        kind: kr.kind,
        manual: kr.manual,
        metric: kr.metric || null,
        value: kr.value,
        baseline: kr.baseline,
        target: kr.target,
        score: kr.score,
        band: kr.band,
        source: kr.source,
        confidence: kr.confidence,
        onPace: kr.on_pace,
      });
    }
  }
  return rows;
}

// --------------------------------------------------------------- roadmap

const LANES = ["now", "next", "later"];
export { LANES as ROADMAP_LANES };

/// `{now: [...], next: [...], later: [...]}`, each item carrying its
/// objectives' own titles and colour indices (for the card's left border,
/// see `objectiveColorVar`) and the mean of those objectives' own scores --
/// `null` when none of them are scored, same "unscored, not zero" rule.
/// `orphan: true` for a roadmap item naming no objective at all -- already
/// an `orphan_roadmap_item` finding, but the roadmap view flags it in place
/// too rather than only in the findings panel below it.
export function roadmapLanes(roadmap, objectives) {
  const list = objectives || [];
  const titleById = new Map(list.map((o) => [o.objective, o.title]));
  const scoreById = new Map(list.map((o) => [o.objective, o.score]));
  const colorById = new Map(list.map((o, i) => [o.objective, i]));

  const lanes = { now: [], next: [], later: [] };
  for (const item of roadmap || []) {
    const ids = item.objectives || [];
    const objs = ids.map((id) => ({
      id,
      title: titleById.has(id) ? titleById.get(id) : id,
      colorIndex: colorById.has(id) ? colorById.get(id) : null,
    }));
    const scores = ids.map((id) => scoreById.get(id)).filter((s) => s !== null && s !== undefined);
    const progress = scores.length ? scores.reduce((a, b) => a + b, 0) / scores.length : null;
    const lane = LANES.includes(item.lane) ? item.lane : "later";
    lanes[lane].push({
      id: item.id,
      title: item.title,
      why: item.why || null,
      scope: item.scope || null,
      objectives: objs,
      orphan: objs.length === 0,
      progress,
    });
  }
  return lanes;
}

/// A small, fixed categorical palette -- `--t1`..`--t7`, the same eight-hue
/// set `terminal.js` already reads for its own ANSI-like colours, minus
/// `--t0` (near-black, unusable as a card accent). Defined identically in
/// both themes (`app.css`'s theme-agnostic `:root` block), so an objective
/// keeps the same colour across a theme switch. A cycle is capped at five
/// objectives (`FindingKind::TooManyObjectives`), well inside these seven.
const OBJECTIVE_COLOR_TOKENS = ["--t1", "--t2", "--t3", "--t4", "--t5", "--t6", "--t7"];
export function objectiveColorVar(index) {
  const n = OBJECTIVE_COLOR_TOKENS.length;
  const i = ((Number(index) || 0) % n + n) % n;
  return `var(${OBJECTIVE_COLOR_TOKENS[i]})`;
}

// ----------------------------------------------------------- strategy map

/// Objectives grouped into cascade layers by `aligns_to` depth: layer 0 is
/// every objective with no parent (or a parent outside the asked scope --
/// see `danglingAlignsTo`), layer 1 aligns to one of those, and so on.
/// Defensive against a cycle in `aligns_to` (already a `CyclicAlignsTo`
/// finding at load time, but this still owes a deterministic answer rather
/// than recursing forever): a node already being resolved on the current
/// path reads as depth 0 rather than looping.
export function objectiveLayers(objectives) {
  const list = objectives || [];
  const byId = new Map(list.map((o) => [o.objective, o]));
  const depth = new Map();

  function depthOf(id, path) {
    if (depth.has(id)) return depth.get(id);
    const o = byId.get(id);
    const parent = o && o.aligns_to;
    if (!parent || !byId.has(parent) || path.has(id)) {
      depth.set(id, 0);
      return 0;
    }
    path.add(id);
    const d = 1 + depthOf(parent, path);
    depth.set(id, d);
    return d;
  }
  for (const o of list) depthOf(o.objective, new Set());

  const maxDepth = list.reduce((m, o) => Math.max(m, depth.get(o.objective) || 0), 0);
  const layers = Array.from({ length: maxDepth + 1 }, () => []);
  for (const o of list) layers[depth.get(o.objective) || 0].push(o);
  return layers;
}

/// `{from, to}` pairs worth a drawn connector: `from` aligns to `to`, and
/// `to` is itself one of the objectives on screen (same scope-filtered set).
export function alignsToEdges(objectives) {
  const list = objectives || [];
  const ids = new Set(list.map((o) => o.objective));
  return list.filter((o) => o.aligns_to && ids.has(o.aligns_to)).map((o) => ({ from: o.objective, to: o.aligns_to }));
}

/// An objective whose `aligns_to` names something not among the objectives
/// on screen -- not a finding (the parent is real, just outside the asked
/// scope's own filter), so the map draws a chip ("aligns to X -- outside
/// this scope") instead of a connector nothing can reach.
export function danglingAlignsTo(objectives) {
  const list = objectives || [];
  const ids = new Set(list.map((o) => o.objective));
  return list.filter((o) => o.aligns_to && !ids.has(o.aligns_to)).map((o) => ({ objective: o.objective, alignsTo: o.aligns_to }));
}

/// A smooth vertical connector from the bottom-centre of `fromRect` (the
/// parent, drawn above) to the top-centre of `toRect` (the child, drawn
/// below), both `{left, top, width, height}` in the same coordinate space as
/// `containerRect` -- an absolutely-positioned overlay `<svg>` drawn from
/// measured `getBoundingClientRect()`s is the only way to connect two cards
/// a CSS grid placed, so this is pure geometry only: `goals.js` alone calls
/// `getBoundingClientRect`.
export function connectorPath(fromRect, toRect, containerRect) {
  const ox = containerRect.left;
  const oy = containerRect.top;
  const x1 = fromRect.left + fromRect.width / 2 - ox;
  const y1 = fromRect.top + fromRect.height - oy;
  const x2 = toRect.left + toRect.width / 2 - ox;
  const y2 = toRect.top - oy;
  const midY = (y1 + y2) / 2;
  return `M ${r1(x1)} ${r1(y1)} C ${r1(x1)} ${r1(midY)}, ${r1(x2)} ${r1(midY)}, ${r1(x2)} ${r1(y2)}`;
}
function r1(n) {
  return Math.round(n * 10) / 10;
}

// --------------------------------------------------------------- orbit

/// Past this many key results the radial view stops being readable (the
/// issue's own number) -- a showpiece, not the working view, so it hides
/// itself with a note rather than drawing an unreadable wheel.
export const ORBIT_MAX_KRS = 20;

/// A fixed radial layout: the vision sits at the centre (drawn by
/// `goals.js` from `direction.vision` directly, not a node here), objectives
/// on an inner ring spaced evenly by index, and each objective's own key
/// results spread across a small arc straddling their objective's own
/// angle on an outer ring. Deterministic from the objectives' own order (no
/// randomness needed, unlike `knowledge-graph.js`'s force layout -- a
/// strategy's cascade is a fixed hierarchy, not an arbitrary graph).
/// `{tooMany: true}` alone, no nodes, once `totalKrs` passes
/// [`ORBIT_MAX_KRS`].
export function orbitLayout(objectives, { width = 640, height = 640 } = {}) {
  const list = objectives || [];
  const totalKrs = list.reduce((n, o) => n + (o.key_results || []).length, 0);
  if (totalKrs > ORBIT_MAX_KRS) return { tooMany: true, totalKrs, nodes: [], edges: [] };

  const cx = width / 2;
  const cy = height / 2;
  const objR = Math.min(width, height) * 0.28;
  const krR = Math.min(width, height) * 0.46;
  const n = list.length || 1;
  const nodes = [];
  const edges = [];

  list.forEach((o, i) => {
    const angle = (2 * Math.PI * i) / n - Math.PI / 2;
    const ox = cx + objR * Math.cos(angle);
    const oy = cy + objR * Math.sin(angle);
    nodes.push({ id: o.objective, kind: "objective", title: o.title, x: r1(ox), y: r1(oy), angle, score: o.score });

    const krs = o.key_results || [];
    const m = krs.length || 1;
    const spread = Math.min((2 * Math.PI) / n, 0.5);
    krs.forEach((kr, j) => {
      const a = m === 1 ? angle : angle - spread / 2 + (spread * j) / (m - 1);
      const kx = cx + krR * Math.cos(a);
      const ky = cy + krR * Math.sin(a);
      nodes.push({ id: kr.kr, kind: "kr", title: kr.title, x: r1(kx), y: r1(ky), angle: a, band: kr.band, krKind: kr.kind });
      edges.push({ from: o.objective, to: kr.kr });
    });
  });

  return { tooMany: false, totalKrs, nodes, edges, cx, cy };
}

// -------------------------------------------------------------- findings

/// The wire's `FindingKind` values (`#[serde(rename_all = "snake_case")]` in
/// `factory_core::goals`), translated the way `policy-model.js`'s own
/// `FINDING_LABELS` translates its. A kind this map has never heard of falls
/// back to its raw wire spelling rather than hiding it.
const FINDING_LABELS = {
  parse_failed: "Catalogue failed to parse",
  stem_mismatch: "Cycle id does not match the file name",
  duplicate_id: "Duplicate id",
  inverted_dates: "Cycle ends before it starts",
  overlapping_cycles: "Two cycles' windows overlap",
  vanity_key_result: "Key result names neither a metric nor manual",
  conflicting_key_result: "Key result names both a metric and manual",
  unknown_metric: "Names a metric nothing recognizes",
  unavailable_metric: "Names a metric that cannot be computed yet",
  objective_without_key_results: "Objective has no key results",
  too_many_objectives: "More than five objectives in one cycle",
  orphan_roadmap_item: "Roadmap item names no objective",
  unknown_roadmap_objective: "Roadmap item names an objective this cycle does not have",
  unknown_aligns_to: "aligns_to names an objective this cycle does not have",
  cyclic_aligns_to: "aligns_to loops back on itself",
  bad_id_shape: "Id is not lowercase, digits and hyphens",
  wrong_direction: "Baseline/target move the wrong way for this metric",
};
export function findingLabel(kind) {
  return FINDING_LABELS[kind] || kind;
}

export function findingsByKind(findings) {
  const groups = new Map();
  for (const f of findings || []) {
    if (!groups.has(f.kind)) groups.set(f.kind, []);
    groups.get(f.kind).push(f);
  }
  return groups;
}

// ------------------------------------------------------------- check-ins

/// The body `POST /api/goals/checkins` expects, from the form's own raw
/// string values -- thrown sentences a person can act on for the checks
/// worth catching before the round trip; `0..=10` and "finite" are the
/// server's own refusal (`Engine::goals_checkin`), mirrored here so a typo
/// never has to make a request to find out.
export function checkinBody(kr, values) {
  const value = Number(values.value);
  if (values.value === "" || values.value === null || values.value === undefined || !Number.isFinite(value)) {
    throw new Error("Value is required and must be a number.");
  }
  const confidence = Number(values.confidence);
  if (!Number.isInteger(confidence) || confidence < 0 || confidence > 10) {
    throw new Error("Confidence must be a whole number from 0 to 10.");
  }
  const note = String(values.note ?? "").trim();
  const body = { kr, value, confidence };
  if (note) body.note = note;
  return body;
}

// -------------------------------------------------------------- view mode

const VIEW_MODES = ["map", "roadmap", "orbit", "table"];
/// A view mode read back from `localStorage` (or a stray hash segment) is
/// untrusted input -- anything else collapses to the landing view rather
/// than showing a blank segmented control with nothing selected.
export function normalizeViewMode(mode) {
  return VIEW_MODES.includes(mode) ? mode : "map";
}

// ------------------------------------------------------------- empty state

/// Whether there is anything authored at all -- `direction.yaml` missing (or
/// unparseable) is the one condition worth a dedicated empty state pointing
/// at how to write one, rather than a strategy map with nothing on it.
export function hasDirection(goalsReport) {
  return !!(goalsReport && goalsReport.direction);
}
