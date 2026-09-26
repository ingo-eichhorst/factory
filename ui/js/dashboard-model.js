//! The dashboard's tile vocabulary (#163, phase 2 of the layout epic #150):
//! what a tile is, the fixed sizes it can be, the fixed view ids it can
//! name, and the default layout that reproduces today's page. Pure -- no
//! DOM, no imports of `core.js` -- so it can be tested without a page and
//! read by whatever eventually validates a scope's own `dashboard:` config
//! (#150 phase 4) the same way it is read here.
//!
//! A tile is `{ view: "<id>", size: "<s|m|l|xl>" }` or, from phase 3 on,
//! `{ metric: "<registry id>", size: "<s|m|l|xl>" }` -- the same two shapes
//! #150's design section 4 already settled on for the `dashboard:` config
//! block, so a tile read here and a tile read from that YAML are the same
//! object. Exactly one of `view`/`metric` names the tile; `size` is always
//! present. There is deliberately no third field for a query, a filter or a
//! group-by (#150 design §8): a tile is a reference into a fixed catalogue,
//! never a question of its own.

/// The two shapes a tile comes in. A `metric` tile binds a registry
/// `MetricId` (#150 §3); none render yet (excluded this phase), so
/// `DEFAULT_DASHBOARD` below has none, but the vocabulary and validator
/// already know the shape so phase 3 does not have to touch this file's
/// contract, only `dashboard.js`'s renderer map.
export const TILE_KINDS = Object.freeze(["metric", "view"]);

/// Sizes name a span on a 12-column row, not a pixel width -- the same
/// indirection a metric's `unit` gives a number. `packRows` below is what
/// turns a span into an actual row; nothing here lays anything out.
///
/// `l` and `m` are fixed by today's page: the Throughput/On-the-line row is
/// a 2fr/1fr CSS grid (`.drow` in `app.css`), i.e. a 2:1 split, and `8:4` is
/// the one split on a 12-column row that both matches that ratio exactly
/// and leaves `l + m === 12` so the pair fills a row on its own -- see
/// `DEFAULT_DASHBOARD` and `dashboard.js`'s `renderDashboard`, where that
/// identity is what makes today's two-card row fall out of `packRows`
/// rather than being hard-coded. `xl` is a full row. `s` renders nothing
/// this phase (no metric tile does yet) but is fixed at `2` now because
/// that is what reproduces today's six-across KPI row exactly (`6 × 2 =
/// 12`) once phase 3 splits the `kpis` view tile into one metric tile per
/// figure -- so a size chosen for a tile that does not exist yet is still
/// chosen for a reason that already exists.
export const SIZES = Object.freeze({
  s: Object.freeze({ cols: 2 }),
  m: Object.freeze({ cols: 4 }),
  l: Object.freeze({ cols: 8 }),
  xl: Object.freeze({ cols: 12 }),
});

export const GRID_COLUMNS = 12;

/// The view-tile vocabulary #150's design settles on for the whole epic --
/// a `view` tile reads one existing endpoint, same as today, never a query.
/// Phase 2 renders five of them (`RENDERABLE_VIEW_IDS` below); the rest are
/// reserved names for phase 3's new cards (agent hours, occupancy, inbox,
/// compliance, cost) so they are not invented ad hoc when they are built.
export const EPIC_VIEW_IDS = Object.freeze([
  "throughput",
  "on_the_line",
  "production_year",
  "by_scope",
  "agent_hours_by_scope",
  "agent_hours_by_agent",
  "occupancy_strip",
  "inbox",
  "compliance",
  "cost",
]);

/// `kpis` is not in #150's own vocabulary above -- it is this phase's
/// stopgap for today's KPI row (six figures read off `state.tasks` and
/// `/api/production`), most likely retired once phase 3 gives each figure
/// its own metric tile. Named clearly so nothing mistakes it for part of
/// the epic's design.
export const VIEW_IDS = Object.freeze(["kpis", ...EPIC_VIEW_IDS]);

/// What phase 2 actually renders, in the order `DEFAULT_DASHBOARD` uses --
/// see `dashboard.js`'s `VIEW_RENDERERS`, which has exactly these keys.
export const RENDERABLE_VIEW_IDS = Object.freeze([
  "kpis",
  "throughput",
  "on_the_line",
  "production_year",
  "by_scope",
]);

/// `"view"` or `"metric"` for a well-shaped tile, `null` for anything else
/// (including one naming both or neither). Used by `validateTile` and by
/// anything that needs to branch on kind without repeating the `in` checks.
export function tileKind(tile) {
  if (!tile || typeof tile !== "object") return null;
  const hasView = Object.prototype.hasOwnProperty.call(tile, "view");
  const hasMetric = Object.prototype.hasOwnProperty.call(tile, "metric");
  if (hasView === hasMetric) return null; // both or neither -- not a tile
  return hasView ? "view" : "metric";
}

/// The columns a tile's `size` spans, or `null` for an unknown size --
/// distinct from 0 so a caller cannot mistake "unknown" for "spans
/// nothing".
export function tileSpan(tile) {
  const size = SIZES[tile && tile.size];
  return size ? size.cols : null;
}

/// Every reason a tile is not well-formed, as a list of messages (empty
/// means valid) -- the same shape `workflow-model.js`'s `validate` returns,
/// so a caller can show them all at once rather than stopping at the first.
/// Structural only: a `metric` id is checked for being a non-empty string,
/// not against the registry, which is server-side and does not exist in
/// this pure module (#150 §4's "unknown metric id ... is a config error" is
/// phase 4's to enforce, once there is a config to load).
export function validateTile(tile) {
  if (!tile || typeof tile !== "object") return ["a tile must be an object"];

  const errors = [];
  const kind = tileKind(tile);
  if (kind === null) {
    errors.push("a tile names exactly one of `view` or `metric`");
  } else if (kind === "view") {
    if (typeof tile.view !== "string" || !VIEW_IDS.includes(tile.view)) {
      errors.push(`unknown view tile ${JSON.stringify(tile.view)}`);
    }
  } else if (kind === "metric") {
    if (typeof tile.metric !== "string" || !tile.metric.trim()) {
      errors.push("a metric tile needs a non-empty metric id");
    }
  }

  if (!Object.prototype.hasOwnProperty.call(SIZES, tile.size)) {
    errors.push(`unknown size ${JSON.stringify(tile.size)}`);
  }

  return errors;
}

/// Every tile's errors, prefixed with its position, flattened into one
/// list -- empty means the whole layout is valid. A dashboard with no
/// tiles is its own error: a layout that draws nothing is not a smaller
/// valid layout, the same way a workflow with no nodes is not one
/// (`workflow-model.js`'s `validate`).
export function validateDashboard(tiles) {
  if (!Array.isArray(tiles) || tiles.length === 0) {
    return ["a dashboard needs at least one tile"];
  }
  return tiles.flatMap((tile, i) => validateTile(tile).map((message) => `tile ${i}: ${message}`));
}

/// Packs tiles into rows on a `GRID_COLUMNS`-wide grid, greedily and in
/// list order: a tile joins the row being built if it still fits, else it
/// starts the next one. An `xl` tile always lands alone, since it already
/// fills the row by itself. This is what makes a tile's `size` an actual
/// layout decision instead of a label nobody reads -- `dashboard.js` never
/// decides by itself that Throughput and On the line share a row; it falls
/// out of `l + m === 12` here, in the one place both numbers live.
///
/// An unknown size spans the whole grid (see `tileSpan`'s `null`, read here
/// as "fill the row") rather than throwing -- `packRows` lays out a
/// dashboard; refusing a bad tile is `validateDashboard`'s job, and a
/// caller that renders without validating first still gets a row per
/// unknown tile instead of a crash.
export function packRows(tiles) {
  const rows = [];
  let row = [];
  let used = 0;

  for (const tile of tiles) {
    const cols = tileSpan(tile) ?? GRID_COLUMNS;
    if (row.length > 0 && used + cols > GRID_COLUMNS) {
      rows.push(row);
      row = [];
      used = 0;
    }
    row.push(tile);
    used += cols;
    if (used >= GRID_COLUMNS) {
      rows.push(row);
      row = [];
      used = 0;
    }
  }
  if (row.length > 0) rows.push(row);
  return rows;
}

/// Today's page, as data: the same cards, in the same order, at the sizes
/// `packRows` needs to reproduce today's rows (`.kpis` alone, Throughput
/// and On the line sharing a `.drow`, then Production year and By scope
/// each alone) -- see `dashboard.js`'s `renderDashboard`. Frozen two levels
/// deep (the list and every tile in it) so nothing downstream can mistake
/// "the default" for something it may edit in place.
export const DEFAULT_DASHBOARD = Object.freeze(
  [
    { view: "kpis", size: "xl" },
    { view: "throughput", size: "l" },
    { view: "on_the_line", size: "m" },
    { view: "production_year", size: "xl" },
    { view: "by_scope", size: "xl" },
  ].map((tile) => Object.freeze(tile))
);
