//! The dashboard's tile vocabulary (#163, phase 2 of the layout epic #150):
//! what a tile is, the fixed sizes it can be, the fixed view ids it can
//! name, and the default layout that reproduces today's page. #162 (phase
//! 3) added the span-aware grid geometry (`rowTemplate`/`rowIsAllSmall`)
//! and made every registry metric and the rest of #150's view catalogue
//! renderable (`isTileRenderable`); the value/unit shaping and the
//! per-endpoint fetch gating those tiles need live in
//! `dashboard-tiles-model.js`, kept separate so this file stays the one
//! place a tile's layout (kind, size, row) is decided. Pure -- no DOM, no
//! imports of `core.js` -- so it can be tested without a page and read by
//! whatever eventually validates a scope's own `dashboard:` config (#150
//! phase 4) the same way it is read here.
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
/// `MetricId` (#150 §3) -- every one of them renders, as of #162, off
/// whatever `/api/metrics` answers for it; `DEFAULT_DASHBOARD` below still
/// has none, since the hard requirement is that the built-in default stays
/// exactly what it always drew.
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
/// rather than being hard-coded. `xl` is a full row. `s` is fixed at `2`
/// because that is what reproduces today's six-across KPI row exactly
/// (`6 × 2 = 12`) if a layout ever splits the `kpis` view tile into one
/// metric tile per figure -- `DEFAULT_DASHBOARD` never does, but a `metric`
/// tile (#162) renders at any size, `s` included.
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

/// What this page can draw a card for, in the order `DEFAULT_DASHBOARD`
/// uses the first five of -- see `dashboard.js`'s `VIEW_RENDERERS`, which
/// has exactly these keys. Phase 2 (#163) built the first five; #162
/// (phase 3) gave the rest of #150's view catalogue their own renderer too,
/// so this is now every `view` id `VIEW_IDS` names -- nothing left for the
/// placeholder to catch but a shape the closed enum itself should already
/// have refused.
export const RENDERABLE_VIEW_IDS = Object.freeze([
  "kpis",
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

// ------------------------------------------------------- phase 4 (#159) reads

/// Which tile list `dashboard.js` should draw: `fetchedTiles` when
/// `GET /api/dashboard` named one, else `DEFAULT_DASHBOARD`. The daemon
/// already answers `tiles: null` for "nothing overrides anything, use the
/// built-in default" (`#159`'s `Config::dashboard_for_scope`), and a fetch
/// that has not resolved yet (`undefined`) or that failed (also read as
/// `null` by the caller) fall back the same way -- so a slow or
/// unreachable daemon still draws today's page, never a blank one. Never
/// re-validates: a layout that reached the wire already passed
/// `Config::validate` server-side, and `DEFAULT_DASHBOARD` is validated by
/// the tests above.
export function resolveDashboard(fetchedTiles) {
  return Array.isArray(fetchedTiles) && fetchedTiles.length > 0 ? fetchedTiles : DEFAULT_DASHBOARD;
}

/// Whether `dashboard.js` can draw `tile` today: a `view` id in
/// `RENDERABLE_VIEW_IDS`, or any well-shaped `metric` tile -- #162 (phase 3)
/// gives every registry metric id a card (`dashboard.js`'s `metricTile`,
/// off whatever `/api/metrics` answered, including its own "not available"
/// reason), so a `metric` tile never needs the registry loaded client-side
/// to know it is drawable. `false` only for a malformed tile (`tileKind`
/// already `null`) or a `view` id outside the vocabulary -- a config or a
/// catalogue ahead of what this build knows how to render, which stays a
/// named placeholder rather than nothing or a crash.
export function isTileRenderable(tile) {
  const kind = tileKind(tile);
  if (kind === "metric") return typeof tile.metric === "string" && tile.metric.trim() !== "";
  return kind === "view" && RENDERABLE_VIEW_IDS.includes(tile.view);
}

// ----------------------------------------------------------- grid (#162)

/// The `.drow` grid's own `grid-template-columns`: one `minmax(0,<cols>fr)`
/// term per tile in `row`, fr-weighted by each tile's own span (`tileSpan`)
/// -- a two-tile row of `[l, m]` (8 and 4) reduces to exactly `8fr / 4fr`,
/// the same ratio as `2fr / 1fr` with the same single gap between them, so
/// it is pixel-identical to `.drow`'s own CSS fallback and this returns
/// `null` for exactly that pair: nothing to override, the default row's
/// markup stays untouched. A row that does not use the full 12 columns (an
/// `m` and an `s` sharing a row a config asks for, say) gets one trailing
/// filler term so the leftover width stays blank rather than stretching the
/// real tiles to fill it -- a true 12-column grid's own behaviour, the same
/// as an unused span in a Bootstrap-style row. `null` for a one-tile row
/// too: `renderTiles` draws that bare, same as before this existed.
export function rowTemplate(row) {
  if (!Array.isArray(row) || row.length < 2) return null;
  const cols = row.map((tile) => tileSpan(tile) ?? GRID_COLUMNS);
  if (cols.length === 2 && cols[0] === SIZES.l.cols && cols[1] === SIZES.m.cols) return null;
  const used = cols.reduce((a, b) => a + b, 0);
  const terms = cols.map((c) => `minmax(0,${c}fr)`);
  if (used < GRID_COLUMNS) terms.push(`minmax(0,${GRID_COLUMNS - used}fr)`);
  return terms.join(" ");
}

/// Whether every tile sharing a row is the smallest size -- `dashboard.js`
/// marks that row `drow-s` so the narrow breakpoint gives it two columns
/// instead of one; a row of six `s` tiles stacked one-per-line reads worse
/// than two-up does. Never true for a one-tile row (nothing to pair) or the
/// default layout, which has no `s` tile at all.
export function rowIsAllSmall(row) {
  return Array.isArray(row) && row.length > 1 && row.every((tile) => tile && tile.size === "s");
}
