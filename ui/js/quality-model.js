//! Pure shaping logic for the L5 Improvement, Quality attributes tab (GitHub
//! issue `#107`, slice 3): everything that turns a `QualityReport` (see
//! `Payload::Quality` in `protocol.rs` and `factory_core::quality`) into the
//! cells, trees, grids and geometry the heatmap, the utility tree, the
//! trade-off matrix, the importance × difficulty grid and the table draw.
//!
//! Nothing here touches `document`, `window`, or `localStorage` -- `quality.js`
//! is the only module that draws anything, so this one is safe to import in
//! a Node test with no DOM at all, the same boundary `goals-model.js`/
//! `policy-model.js` keep. It imports `routeHref` from `scopes.js`, `nodeTail`
//! from `knowledge-graph.js`, and `describeCheck`/`refLinks` from
//! `policy-model.js` -- a check measure *is* a policy check on the wire, so
//! describing one and linking its evidence has exactly one implementation.
//!
//! ## No score, anywhere
//!
//! The daemon deliberately sends none (`QualityReport`'s own doc comment):
//! an attribute is the worst of its scenarios and that is as far as any
//! rollup goes. A heatmap cell is the same rule one step further -- the
//! worst of the attributes a scope declares under that characteristic -- and
//! nothing here ever averages, counts into a percentage, or ranks one scope
//! against another. The issue's guardrail, kept by construction.
//!
//! ## Wire shapes worth naming precisely
//!
//! `ScopeQuality` flattens `ScopeReport`, so a row is `{scope, profiles,
//! attributes, tradeoffs, open_tasks?}` -- and `open_tasks` is skipped when
//! empty, so it is read with a `|| {}` every time. `ScenarioResult` flattens
//! `AppliedScenario`, which flattens `QualityScenario`: the six parts, the
//! measure, `declared_at`, `status`, `reasons`, `value?`, `as_of?`, `refs?`
//! all sit on one object. A measure's `max_age` comes back in the daemon's
//! own spelling (`7d` is written back as `1w`) and is shown as it arrives.
//! Captured from a throwaway daemon rather than read off the Rust -- see
//! `ui/tests/quality-model.test.js`'s header comment.

import { routeHref } from "./scopes.js";
import { nodeTail } from "./knowledge-graph.js";
import { describeCheck, refLinks } from "./policy-model.js";

// ------------------------------------------------------------------ status

/// Best to worst, the same order `ScenarioStatus` derives `Ord` in: `draft`
/// sits above `no_data` because a scenario nobody has measured yet is less
/// alarming than a measure nobody can read -- but neither is ever green.
export const STATUS_ORDER = ["met", "draft", "no_data", "stale", "not_met"];

const STATUS_LABELS = { met: "met", draft: "draft", no_data: "no data", stale: "stale", not_met: "not met" };
export function statusLabel(status) {
  return STATUS_LABELS[status] || String(status || "");
}

/// The worst of `statuses`, by `STATUS_ORDER`. With nothing to roll up it is
/// `draft` -- `ScenarioStatus::worst`'s own rule: a stated concern with no
/// specification yet, never `met`. An unknown spelling ranks worst, so a
/// status a newer daemon invents is never read as good news.
export function worstStatus(statuses) {
  let worst = null;
  let rank = -1;
  for (const s of statuses || []) {
    const r = STATUS_ORDER.indexOf(s);
    const eff = r < 0 ? STATUS_ORDER.length : r;
    if (eff > rank) { rank = eff; worst = s; }
  }
  return worst === null ? "draft" : worst;
}

/// Whether a scenario offers "Create task". `not_met` and `stale` always
/// do. `no_data` does when a task could close the gap -- the bench was
/// never run, the fitness-function task never finished -- and not when only
/// an edit to the profile could: an `attestation` check measure, a
/// `quality.*` metric (circular), or a metric the registry does not know or
/// cannot compute yet, which the report names in an `unknown_metric`/
/// `unavailable_metric`/`self_referential_metric` finding. That is the
/// daemon's own `unfixable_by_a_task` rule read off the wire; the daemon's
/// refusal stays the guard for anything this misjudges, and lands inline
/// with its reason. `met` and `draft` never do (the daemon refuses both).
const UNFIXABLE_FINDINGS = ["unknown_metric", "unavailable_metric", "self_referential_metric"];
export function canRemediate(scenario, findings) {
  const status = scenario && scenario.status;
  if (status === "not_met" || status === "stale") return true;
  if (status !== "no_data") return false;
  const m = scenario.measure;
  if (!m) return false;
  if (m.check) return m.check !== "attestation";
  if (!m.metric || /^quality\./.test(m.metric)) return false;
  const named = new RegExp(`(^|\\s)${m.metric.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(\\s|,|:|$)`);
  return !(findings || []).some((f) => UNFIXABLE_FINDINGS.includes(f.kind) && named.test(f.detail || ""));
}

// ------------------------------------------------------------------ levels

const LEVEL_RANK = { L: 1, M: 2, H: 3 };
export const LEVELS = ["H", "M", "L"];

/// The highest of `levels` (H > M > L), or `null` for none.
export function maxLevel(levels) {
  let best = null;
  for (const l of levels || []) {
    if (!LEVEL_RANK[l]) continue;
    if (best === null || LEVEL_RANK[l] > LEVEL_RANK[best]) best = l;
  }
  return best;
}

const LEVEL_WORDS = { H: "high", M: "medium", L: "low" };
export function levelWord(level) {
  return LEVEL_WORDS[level] || String(level || "");
}

/// `(H,M)` -- ATAM's own shorthand, importance first.
export function idTag(attribute) {
  return `(${attribute.importance},${attribute.difficulty})`;
}

// --------------------------------------------------------------- catalogue

/// `{title, sub}` for an attribute id `characteristic[.sub]`, from the
/// report's own compiled-in catalogue. Falls back to the raw id segments
/// when the catalogue does not know it -- the daemon drops an unknown
/// attribute with a finding, so this only happens across a version skew.
export function attributeTitles(id, catalogue) {
  const [cid, sid] = String(id || "").split(".");
  const c = (catalogue || []).find((x) => x.id === cid);
  const s = c && sid ? (c.subs || []).find((x) => x.id === sid) : null;
  return {
    characteristic: cid,
    title: c ? c.title : cid,
    sub: sid ? (s ? s.title : sid) : null,
    standard: s ? s.standard : null,
  };
}

/// "Security › Integrity", or just "Security" for a bare characteristic.
export function attributeLabel(id, catalogue) {
  const t = attributeTitles(id, catalogue);
  return t.sub ? `${t.title} › ${t.sub}` : t.title;
}

// ----------------------------------------------------------------- heatmap

/// The landing view: one row per scope the report lists (every one binds at
/// least one profile -- a scope binding none is omitted server-side), one
/// column per ISO 25010 characteristic in the standard's own order.
///
/// A cell is `declared: false` when the scope states no attribute under that
/// characteristic -- "not a stated concern", which the view draws blank and
/// visibly unlike any status colour. Otherwise its `status` is the worst of
/// those attributes' own rollups and `importance` the highest of theirs (the
/// border weight): one H attribute under a characteristic makes the cell an
/// H cell, whatever else sits beside it.
export function heatmapRows(report) {
  const columns = ((report && report.catalogue) || []).map((c) => ({ id: c.id, title: c.title }));
  const rows = ((report && report.scopes) || []).map((sq) => {
    const cells = columns.map((col) => {
      const attrs = (sq.attributes || []).filter((a) => (a.characteristic || String(a.id).split(".")[0]) === col.id);
      if (!attrs.length) return { characteristic: col.id, declared: false, status: null, importance: null, attributes: [] };
      return {
        characteristic: col.id,
        declared: true,
        status: worstStatus(attrs.map((a) => a.status)),
        importance: maxLevel(attrs.map((a) => a.importance)),
        attributes: attrs.map((a) => a.id),
      };
    });
    return { scope: sq.scope, profiles: sq.profiles || [], cells };
  });
  return { columns, rows };
}

/// What a screen reader (and a hover title) hears for one cell -- the
/// colour and border weight say it at a glance, this says it in words.
export function cellLabel(scope, column, cell) {
  if (!cell.declared) return `${scope}, ${column.title}: not declared`;
  return `${scope}, ${column.title}: ${statusLabel(cell.status)}, ${levelWord(cell.importance)} importance (${cell.attributes.join(", ")})`;
}

// ------------------------------------------------------------ utility tree

/// One scope's ATAM utility tree: characteristic → attribute (its
/// refinement, when it names a sub-characteristic) → scenario, in the
/// order the profile chain declared them. Each characteristic node carries
/// the worst of its attributes' statuses, the same rule a heatmap cell
/// follows, so the tree and the heatmap can never disagree about a cell.
export function utilityTree(scopeQuality, catalogue) {
  const groups = new Map();
  for (const a of (scopeQuality && scopeQuality.attributes) || []) {
    const t = attributeTitles(a.id, catalogue);
    const cid = a.characteristic || t.characteristic;
    if (!groups.has(cid)) groups.set(cid, { characteristic: cid, title: t.title, attributes: [] });
    groups.get(cid).attributes.push({ ...a, title: t.sub || t.title, standard: t.standard });
  }
  // The catalogue's own order, so every scope's tree reads in the same
  // sequence as the heatmap's columns; anything the catalogue does not know
  // goes last rather than vanishing.
  const order = ((catalogue || []).map((c) => c.id));
  const rank = (id) => { const i = order.indexOf(id); return i < 0 ? order.length : i; };
  return [...groups.values()]
    .sort((x, y) => rank(x.characteristic) - rank(y.characteristic))
    .map((g) => ({ ...g, status: worstStatus(g.attributes.map((a) => a.status)) }));
}

/// The remediation task already open for a scenario, by the daemon's own
/// `<attribute>/<scenario>` key (`ScopeQuality::open_tasks`), or `null`.
export function openTaskFor(scopeQuality, attribute, scenario) {
  const map = (scopeQuality && scopeQuality.open_tasks) || {};
  return map[`${attribute}/${scenario}`] || null;
}

/// The body `POST /api/quality/remediate` expects.
export function remediateBody(scope, attribute, scenario) {
  return { scope, attribute, scenario };
}

/// A task-modal link for a bare task id -- the same href `refLinks` builds
/// for a check measure's own task ref, so the two can never drift apart.
export function taskHref(scope, taskId) {
  return refLinks(scope, [{ kind: "task", id: taskId }])[0].href;
}

// ---------------------------------------------------------------- sentence

/// `12`, `0.9`, `0.333` -- three significant digits at most, never a
/// trailing run of zeros, and a percentage never invented (the report
/// carries no units; `0.33` is honest where `33%` would be a guess about
/// what the metric measures).
export function formatNumber(v) {
  if (v === null || v === undefined || !Number.isFinite(v)) return "—";
  if (Number.isInteger(v)) return String(v);
  const abs = Math.abs(v);
  const digits = abs >= 100 ? 0 : abs >= 10 ? 1 : abs >= 1 ? 2 : 3;
  return String(Number(v.toFixed(digits)));
}

/// The response measure in words. A metric measure's bounds are inclusive
/// (`quality::evaluate`), hence `≥`/`≤`; a band is both. A check measure is
/// described by `policy-model.js`'s `describeCheck`, since it is one.
export function describeMeasure(measure) {
  if (!measure) return null;
  if (measure.metric) {
    const hasAbove = measure.above !== undefined && measure.above !== null;
    const hasBelow = measure.below !== undefined && measure.below !== null;
    let bound = "";
    if (hasAbove && hasBelow) bound = ` between ${formatNumber(measure.above)} and ${formatNumber(measure.below)}`;
    else if (hasAbove) bound = ` ≥ ${formatNumber(measure.above)}`;
    else if (hasBelow) bound = ` ≤ ${formatNumber(measure.below)}`;
    const age = measure.max_age ? `, no older than ${measure.max_age}` : "";
    return `${measure.metric}${bound}${age}`;
  }
  if (measure.check) return describeCheck(measure);
  return null;
}

/// Whether a measure is a *continual* one (a threshold on a registry
/// metric) or a *triggered* one (a policy-style check) -- the fitness
/// function distinction the issue draws.
export function measureKind(measure) {
  if (!measure) return null;
  return measure.metric ? "continual" : measure.check ? "triggered" : null;
}

/// A scenario's six parts as one sentence, in segments so the view can set
/// every authored part apart (`part` names which of the six it is) without
/// this module writing any markup:
///
///   When <stimulus> (from <source>) on <artifact> during <environment>,
///   <response>. Measured by <measure>.
///
/// Every part is optional on the wire except the id, so the sentence
/// degrades: a scenario with only a measure reads "Measured by …", one with
/// no measure ends "No response measure yet — a draft, never met." rather
/// than pretending to a measure it does not have.
export function scenarioSentence(s) {
  const seg = [];
  const text = (t) => seg.push({ text: t });
  const part = (name, value) => seg.push({ text: value, part: name });
  const has = (k) => typeof s[k] === "string" && s[k].trim() !== "";

  const lead = has("stimulus") || has("source") || has("artifact") || has("environment");
  if (lead) {
    text("When ");
    if (has("stimulus")) part("stimulus", s.stimulus);
    else text("something happens");
    if (has("source")) { text(" (from "); part("source", s.source); text(")"); }
    if (has("artifact")) { text(" on "); part("artifact", s.artifact); }
    if (has("environment")) { text(" during "); part("environment", s.environment); }
    text(has("response") ? ", " : ".");
  }
  if (has("response")) {
    const r = s.response.trim();
    part("response", lead ? r : r.charAt(0).toUpperCase() + r.slice(1));
    text(/[.!?]$/.test(r) ? "" : ".");
  }
  const m = describeMeasure(s.measure);
  if (m) {
    if (seg.length) text(" ");
    text("Measured by ");
    part("measure", m);
    text(".");
  } else {
    if (seg.length) text(" ");
    text("No response measure yet — a draft, never met.");
  }
  return seg;
}

/// The same sentence flattened to plain text, for the table and a title.
export function sentenceText(s) {
  return scenarioSentence(s).map((x) => x.text).join("");
}

// ------------------------------------------------------------ bullet chart

/// The history behind a scenario's metric, oldest first, as `{day, value}`
/// with `day` a whole-day index (UTC days since the epoch) so a gap in the
/// series is a gap on the axis -- from the report's own `series` (only ever
/// present for the production metrics a scenario reads, never invented for
/// one without), or `null`. A point with no finite value is dropped here
/// and shows as the gap it is.
export function seriesPoints(report, metric) {
  if (!metric) return null;
  const s = ((report && report.series) || []).find((x) => x.id === metric);
  if (!s || !s.points || !s.points.length) return null;
  const out = [];
  for (const [date, value] of s.points) {
    const ms = Date.parse(`${String(date).slice(0, 10)}T00:00:00Z`);
    if (Number.isFinite(ms) && Number.isFinite(value)) out.push({ day: Math.round(ms / 86400000), value });
  }
  out.sort((a, b) => a.day - b.day);
  return out.length ? out : null;
}

/// Just the values of `seriesPoints`, for the value axis.
export function seriesValues(report, metric) {
  const pts = seriesPoints(report, metric);
  return pts ? pts.map((p) => p.value) : null;
}

/// The one value axis a scenario card's bullet chart and sparkline share:
/// it holds zero, the value, every bound and every point in the history, so
/// the threshold tick on the bar and the dashed threshold line through the
/// sparkline sit at the same height of the same scale. A domain that fits
/// inside `0..1` is drawn as exactly `0..1` -- most measures here are
/// ratios, and a chart whose right edge moved with the data would make 0.33
/// of 0..0.4 look nearly full.
export function valueDomain(measure, value, history) {
  const vals = [0];
  if (Number.isFinite(value)) vals.push(value);
  if (measure && Number.isFinite(measure.above)) vals.push(measure.above);
  if (measure && Number.isFinite(measure.below)) vals.push(measure.below);
  for (const v of history || []) if (Number.isFinite(v)) vals.push(v);
  const lo = Math.min(...vals);
  const hi = Math.max(...vals);
  if (lo >= 0 && hi <= 1) return [0, 1];
  const span = hi - lo || 1;
  return [lo, hi + span * 0.1];
}

/// A bullet chart's geometry in a `width × height` box: the pass region
/// (where the value has to land -- `≥ above`, `≤ below`, or the band between
/// them), the value bar from the axis' low end, and one tick per bound.
/// `null` for a scenario with no metric measure or no value to draw -- a
/// check measure has a verdict, not a quantity, and a missing value is
/// never drawn as a zero.
export function bulletGeometry(measure, value, history, { width = 240, height = 28 } = {}) {
  if (!measure || !measure.metric || !Number.isFinite(value)) return null;
  const [lo, hi] = valueDomain(measure, value, history);
  const x = (v) => ((Math.min(Math.max(v, lo), hi) - lo) / (hi - lo)) * width;
  const hasAbove = Number.isFinite(measure.above);
  const hasBelow = Number.isFinite(measure.below);
  const passFrom = hasAbove ? x(measure.above) : 0;
  const passTo = hasBelow ? x(measure.below) : width;
  const ticks = [];
  if (hasAbove) ticks.push({ x: x(measure.above), value: measure.above, kind: "above" });
  if (hasBelow) ticks.push({ x: x(measure.below), value: measure.below, kind: "below" });
  const barH = Math.round(height * 0.36);
  // The bar grows from zero, which `valueDomain` always holds -- so a
  // negative value reads leftwards of its zero, never as a positive length
  // from the axis' low end.
  const x0 = x(0);
  const xv = x(value);
  return {
    width,
    height,
    domain: [lo, hi],
    pass: { x: passFrom, w: Math.max(passTo - passFrom, 0) },
    bar: { x: Math.min(x0, xv), y: (height - barH) / 2, w: Math.abs(xv - x0), h: barH },
    ticks,
  };
}

/// A sparkline on the bullet chart's own value axis (`valueDomain`), with
/// a dashed line at every bound -- so "above the line" means the same thing
/// in both pictures. `points` are `seriesPoints`' `{day, value}`: x is the
/// day, spread from the first to the last, so a missing day is space on the
/// axis rather than squeezed out, and the line breaks there -- `segments`
/// holds one polyline per unbroken run of days, and a lone day between two
/// gaps is a segment of one (drawn as a dot). `null` with fewer than two
/// points: one point is a value, not a trend.
export function sparkGeometry(points, domain, { width = 240, height = 30 } = {}) {
  if (!points || points.length < 2 || !domain) return null;
  const [lo, hi] = domain;
  const y = (v) => height - ((Math.min(Math.max(v, lo), hi) - lo) / (hi - lo || 1)) * height;
  const first = points[0].day;
  const span = points[points.length - 1].day - first || 1;
  const x = (day) => ((day - first) / span) * width;
  const segments = [];
  let run = [];
  for (let i = 0; i < points.length; i++) {
    if (i > 0 && points[i].day - points[i - 1].day > 1) { segments.push(run); run = []; }
    run.push(`${x(points[i].day).toFixed(1)},${y(points[i].value).toFixed(1)}`);
  }
  segments.push(run);
  return { width, height, segments, count: points.length, days: span + 1, y };
}

// ---------------------------------------------------------------- tradeoffs

/// The attribute × attribute trade-off grid for one scope. Axes are the
/// scope's declared attributes in their declared order, plus -- at the end,
/// flagged `declared: false` -- any attribute a trade-off names that the
/// scope does not declare (already an `undeclared_tradeoff_attribute`
/// finding; drawn rather than dropped so the point is still visible).
/// `cells` is keyed by both orders of a pair (`a|b` and `b|a`), both
/// holding the one same list, so the grid is symmetric without the view
/// doing the arithmetic twice.
export function tradeoffMatrix(scopeQuality) {
  const declared = ((scopeQuality && scopeQuality.attributes) || []).map((a) => a.id);
  const axes = declared.map((id) => ({ id, declared: true }));
  const seen = new Set(declared);
  const cells = new Map();
  for (const t of (scopeQuality && scopeQuality.tradeoffs) || []) {
    const [a, b] = t.between || [];
    if (!a || !b) continue;
    for (const id of [a, b]) {
      if (!seen.has(id)) { seen.add(id); axes.push({ id, declared: false }); }
    }
    // One list under both orders of the pair, not two copies of it.
    let list = cells.get(`${a}|${b}`);
    if (!list) {
      list = [];
      cells.set(`${a}|${b}`, list);
      cells.set(`${b}|${a}`, list);
    }
    list.push(t);
  }
  return { axes, cells, count: ((scopeQuality && scopeQuality.tradeoffs) || []).length };
}

/// Where a trade-off's `decision` leads. A knowledge-vault page --
/// `knowledge/x.md` or `.factory/knowledge/x.md`, the vault being
/// `.factory/knowledge/` -- deep-links into the Knowledge tab by its page
/// id (the vault-relative path without `.md`, `knowledge.rs`'s own
/// definition); an `http(s)` URL opens as itself; anything else, a bare
/// `docs/adr.md` included, is shown as it was written and never guessed
/// into a link to a page that may not be in the vault at all.
export function decisionLink(decision, scope) {
  if (!decision) return null;
  const d = String(decision).trim();
  if (/^https?:\/\//i.test(d)) return { kind: "external", label: d, href: d };
  const m = /^(?:\.\/)?(?:\.factory\/)?knowledge\/(.+)\.md$/i.exec(d);
  if (m && m[1]) {
    const tail = nodeTail(m[1]).map(encodeURIComponent).join("/");
    return { kind: "knowledge", label: m[1], href: `${routeHref(scope, "knowledge")}/${tail}` };
  }
  return { kind: "text", label: d, href: null };
}

// ------------------------------------------------- importance × difficulty

/// ATAM's 3×3: importance down (H at the top), difficulty across (L on the
/// left), so the (H,H) corner -- important *and* hard, where architecture
/// attention belongs and whose scenarios should be measured first -- sits
/// top right, `hot: true`. Nine cells always, empty ones included: a grid
/// that dropped its empty cells would stop being a grid.
export function importanceDifficultyGrid(scopeQuality) {
  const attrs = (scopeQuality && scopeQuality.attributes) || [];
  const cells = [];
  for (const importance of ["H", "M", "L"]) {
    for (const difficulty of ["L", "M", "H"]) {
      cells.push({
        importance,
        difficulty,
        hot: importance === "H" && difficulty === "H",
        attributes: attrs.filter((a) => a.importance === importance && a.difficulty === difficulty),
      });
    }
  }
  return cells;
}

// -------------------------------------------------------------------- table

/// The accessible equivalent of every picture above: one row per scenario,
/// across every scope in the report, with nothing the pictures show left
/// out. An attribute with no scenarios still gets a row (scenario `null`),
/// since "declared, unspecified" is a fact the tree shows too.
export function tableRows(report) {
  const rows = [];
  for (const sq of (report && report.scopes) || []) {
    for (const a of sq.attributes || []) {
      const scenarios = a.scenarios && a.scenarios.length ? a.scenarios : [null];
      for (const s of scenarios) {
        rows.push({
          scope: sq.scope,
          attribute: a.id,
          label: attributeLabel(a.id, report.catalogue),
          importance: a.importance,
          difficulty: a.difficulty,
          attributeStatus: a.status,
          scenario: s ? s.id : null,
          kind: s ? s.kind || null : null,
          status: s ? s.status : a.status,
          measure: s ? describeMeasure(s.measure) : null,
          value: s && Number.isFinite(s.value) ? s.value : null,
          reasons: s ? s.reasons || [] : [],
          declaredAt: (s || a).declared_at || null,
        });
      }
    }
  }
  return rows;
}

// ------------------------------------------------------------------ findings

/// Human labels for `quality::FindingKind` -- a kind this map has never
/// heard of falls back to its raw wire spelling rather than hiding it, the
/// same rule `policy-model.js`'s own map follows.
const FINDING_LABELS = {
  parse_failed: "Profile did not parse",
  bad_id_shape: "Id is not lowercase, digits and hyphens",
  unknown_attribute: "Not an ISO 25010 attribute",
  duplicate_id: "Id declared twice",
  unknown_metric: "Names a metric nothing recognizes",
  unavailable_metric: "Names a metric that cannot be computed yet",
  missing_threshold: "Metric measure with no threshold",
  non_finite_threshold: "Threshold is not a finite number",
  impossible_band: "above is over below -- can never be met",
  unknown_daemon_fact: "Unknown daemon fact",
  unknown_secrets_location: "Unknown secrets location",
  bad_tradeoff: "Trade-off between an attribute and itself",
  self_referential_metric: "Measures by a quality.* metric (circular)",
  ambiguous_check_target: "Check names more than one task or workflow",
  untagged_knowledge_check: "Knowledge check whose default tag can never exist",
  missing_profile: "Config names a profile with no file",
  loosening: "A descendant tried to loosen an inherited measure",
  conflicting_override: "A descendant restated something differently",
  too_many_attributes: "More than seven attributes",
  unmeasured_high_importance: "H-importance attribute with no measured scenario",
  undeclared_tradeoff_attribute: "Trade-off names an undeclared attribute",
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

// ------------------------------------------------------------ view + focus

const VIEW_MODES = ["heatmap", "tree", "tradeoffs", "grid", "table"];
/// A view mode read back from `localStorage` is untrusted input -- anything
/// else collapses to the landing heatmap.
export function normalizeViewMode(mode) {
  return VIEW_MODES.includes(mode) ? mode : "heatmap";
}

/// Which scope the per-scope views (tree, trade-offs, grid) show: the one
/// asked for if the report still lists it, else the rail's own selection if
/// that is listed, else the first row. `null` only for an empty report.
export function focusScope(report, wanted, railScope) {
  const names = ((report && report.scopes) || []).map((s) => s.scope);
  if (wanted && names.includes(wanted)) return wanted;
  if (railScope && names.includes(railScope)) return railScope;
  return names.length ? names[0] : null;
}

/// Whether any scope binds a profile -- an empty `scopes` is the one
/// condition worth an empty state that says how to write one.
export function hasProfiles(report) {
  return !!(report && report.scopes && report.scopes.length);
}
