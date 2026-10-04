//! Pure shaping for the dashboard's tile editor (`#160`, phase 5 of `#150`):
//! the catalogue a person searches and adds tiles from, and the small edits
//! -- add, remove, reorder, resize -- `dashboard.js`'s Customise mode makes
//! to a working copy of the tile list before it is sent to the daemon
//! whole. No DOM, no `core.js` import: the same boundary `dashboard-model.js`
//! and `dashboard-tiles-model.js` already keep, so this is safe to test with
//! no page at all and safe for `dashboard-model.js` itself to lean on
//! without a cycle back the other way.
//!
//! Validity is never reinvented here: `validateTile`/`validateDashboard`
//! (`dashboard-model.js`) are the one place a tile or a layout is checked,
//! read the same way whether the layout came off the wire or out of this
//! editor. What is new in this file is the catalogue (`VIEW_IDS` plus
//! whatever `/api/metrics`' `registry` names) and the handful of pure list
//! edits a working copy needs -- nothing about what a tile *is* changes.

import { VIEW_IDS, SIZES, DEFAULT_DASHBOARD, validateTile, validateDashboard } from "./dashboard-model.js";

/// A person-readable label per `view` id, in `VIEW_IDS`' own order --
/// exactly the headings `dashboard.js`'s `VIEW_RENDERERS`/`dcard` calls
/// already draw for each, spelled once here so the catalogue and the page
/// never name a tile two different ways.
export const VIEW_LABELS = Object.freeze({
  kpis: "KPIs",
  throughput: "Throughput",
  on_the_line: "On the line",
  production_year: "The production year",
  by_scope: "By scope",
  agent_hours_by_scope: "Agent hours by scope",
  agent_hours_by_agent: "Agent hours by agent",
  occupancy_strip: "Occupancy strip",
  inbox: "Inbox",
  compliance: "Compliance",
  cost: "Cost",
  important_dates: "Important dates",
});

/// Every catalogue entry: one per `view` id, then one per metric `/api/
/// metrics`' `registry` named -- `{ kind: "view"|"metric", id, title,
/// subtitle }`. `registry` is the same `MetricDefView[]` shape the wire
/// already answers (`id`, `title`, `description`); an absent or empty
/// registry (not fetched yet, or the fetch failed) still gives every view
/// tile, so the catalogue is never empty just because `/api/metrics` is
/// slow -- only the metric half of it is.
export function buildCatalogue(registry) {
  const views = VIEW_IDS.map((id) => ({
    kind: "view",
    id,
    title: VIEW_LABELS[id] || id,
    subtitle: "view",
  }));
  const metrics = (Array.isArray(registry) ? registry : []).map((def) => ({
    kind: "metric",
    id: def.id,
    title: def.title || def.id,
    subtitle: def.description || "metric",
  }));
  return [...views, ...metrics];
}

/// Case-insensitive substring match against an entry's id and title -- the
/// whole catalogue for a blank or whitespace-only query, never a search that
/// returns nothing because nothing was typed yet.
export function searchCatalogue(catalogue, query) {
  const q = (query || "").trim().toLowerCase();
  if (!q) return catalogue;
  return catalogue.filter((entry) => entry.id.toLowerCase().includes(q) || entry.title.toLowerCase().includes(q));
}

/// A catalogue entry turned into the tile shape `dashboard-model.js`'s
/// `tileKind` reads: `{ view: id, size }` or `{ metric: id, size }`, never
/// both keys, so a tile built here is indistinguishable from one that came
/// off the wire.
export function entryToTile(entry, size) {
  return entry.kind === "view" ? { view: entry.id, size } : { metric: entry.id, size };
}

/// The working copy `dashboard.js`'s Customise mode starts from: the
/// fetched layout, copied so editing it never mutates what a later Reset or
/// a failed Save would need to fall back to, or a **copy** of
/// `DEFAULT_DASHBOARD` when nothing overrides anything -- `DEFAULT_DASHBOARD`
/// is frozen two levels deep, so editing it in place would throw. Mirrors
/// `resolveDashboard`'s own fallback rule exactly, just handing back
/// something safe to push onto rather than the frozen source.
export function seedTiles(fetchedTiles) {
  const source = Array.isArray(fetchedTiles) && fetchedTiles.length > 0 ? fetchedTiles : DEFAULT_DASHBOARD;
  return source.map((tile) => ({ ...tile }));
}

/// Appends one tile at `size` (default `m`, the middle of the four) without
/// touching the rest of the list -- add always lands at the end, the same
/// place a person reading top to bottom expects a new row to show up.
export function addTile(tiles, entry, size) {
  return [...tiles, entryToTile(entry, size || "m")];
}

/// Every tile but the one at `index`. Out of range is a no-op copy, never a
/// throw -- a stale index from a redraw racing a click should do nothing,
/// not corrupt the list.
export function removeTile(tiles, index) {
  if (index < 0 || index >= tiles.length) return [...tiles];
  return tiles.filter((_, i) => i !== index);
}

/// Swaps `index` with its neighbour one earlier/later in the list -- the
/// pair `dashboard.js`'s move-up/move-down buttons call. A tile already at
/// the edge it is asked to move past is a no-op copy, same reasoning as
/// `removeTile`'s out-of-range case.
export function moveTile(tiles, index, direction) {
  const target = index + direction;
  if (index < 0 || index >= tiles.length || target < 0 || target >= tiles.length) return [...tiles];
  const next = [...tiles];
  [next[index], next[target]] = [next[target], next[index]];
  return next;
}

export function moveTileUp(tiles, index) {
  return moveTile(tiles, index, -1);
}

export function moveTileDown(tiles, index) {
  return moveTile(tiles, index, 1);
}

/// The tile at `index` with its `size` changed to one of `SIZES`' own keys
/// -- an unknown size is refused (returns the list unchanged) rather than
/// written in, so a working copy can never hold a size `validateTile` would
/// reject; the editor's own size picker only ever offers `SIZES`' keys, so
/// this is a guard against a caller bug, not a path a person can reach.
export function setTileSize(tiles, index, size) {
  if (index < 0 || index >= tiles.length || !Object.prototype.hasOwnProperty.call(SIZES, size)) {
    return [...tiles];
  }
  return tiles.map((tile, i) => (i === index ? { ...tile, size } : tile));
}

/// Whether the working copy is savable as it stands -- `validateDashboard`'s
/// own answer, empty meaning valid. Exposed here too so `dashboard.js` reads
/// one function for "can Save be pressed" without reaching past this module
/// into `dashboard-model.js` for the same fact `validateTile` already named.
export function editorErrors(tiles) {
  return validateDashboard(tiles);
}

/// Reset is offered only when the scope being edited is the very one whose
/// block is currently answering -- editing `demo` while `source` reads
/// `"projects"` means `demo` has nothing of its own to remove yet; offering
/// Reset there would ask the daemon to remove a block that is not there,
/// which it already refuses, but the button should not invite the click in
/// the first place. `null` `source` (the built-in default) or a `source`
/// naming any other scope both read `false`.
export function canReset(source, editingScope) {
  return !!source && !!editingScope && source === editingScope;
}

/// A path's segments, `.` and empty ones dropped -- the same normalisation
/// `scopes.js`'s own `segments()` applies before comparing two paths, needed
/// here for the same reason: a discovered root scope's own `Scope.path` is
/// the literal `.` Rust's `PathBuf::join` never collapses, so `ScopeView.path`
/// for it arrives over the wire as `"<root>/."`, not `"<root>"`. A plain
/// string comparison (or a bare trailing-slash trim) never matches that,
/// which silently makes the root scope unfindable.
function pathSegments(path) {
  return String(path ?? "")
    .split("/")
    .filter((s) => s !== "" && s !== ".");
}

/// Which of `scopes` (`state.scopes`) sits at `root` (`state.root`) itself --
/// the instance root's own configured scope, found by comparing path
/// segments rather than by name, since nothing about a scope's name says
/// where it sits on disk. `null` when `root` is empty/unknown, or when
/// nothing in `scopes` sits there: the instance never opted itself into
/// being a scope at all, so there is no name a write could be given.
export function rootScopeName(scopes, root) {
  if (!String(root ?? "").trim()) return null;
  const want = pathSegments(root);
  const found = (scopes || []).find((s) => {
    const have = pathSegments(s.path);
    return have.length === want.length && have.every((seg, i) => seg === want[i]);
  });
  return found ? found.name : null;
}

// Re-exported so a caller that only needs the editor never has to import
// `dashboard-model.js` too just for the one check it shares with the read
// side.
export { validateTile };
