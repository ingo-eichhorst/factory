//! The dashboard: what is true now, read from `state.tasks` and
//! `state.scopes` the way it always was, plus what this factory has been
//! doing, read from `/api/production` -- the one aggregate over run history
//! the daemon serves (`production.rs`). One request per window or scope
//! change, not one per card: every history-backed card below reads the same
//! answer, so the throughput chart and the sparklines under the KPIs can
//! never disagree with each other.
//!
//! Which tiles to draw, and in what order, is a third, separately-scoped
//! read: `GET /api/dashboard?scope=` (`#159`), fetched alongside
//! `/api/production` on load and on a scope change, but never on a window
//! change -- a layout does not depend on how far back the history cards
//! look. `resolveDashboard` (`dashboard-model.js`) is where "nothing
//! overrides anything, or the fetch failed" turns into `DEFAULT_DASHBOARD`,
//! so this file never has to ask which case it is in.
//!
//! Two fixed calendar facts decided once in the endpoint and never re-decided
//! here: a run is **finished** when `ended_at` is set, bucketed by
//! `ended_at`; a finished run is **scrapped** if it ended `failed` or
//! `cancelled`, and **reworked** if it re-attempts work that did not succeed
//! -- a retry, or a manual/workflow re-run of a task whose previous run
//! failed or was cancelled. Not `attempt > 1`: a scheduled task's every
//! firing bumps its own `attempt`, so that alone would read a healthy
//! recurring task as almost entirely rework -- see `production.rs`'s module
//! doc comment for the real rule (`is_rework`). Reworked and scrapped are
//! read separately and neither implies the other: a run started again is not
//! evidence anyone rejected the one before, only that it was tried again.
//!
//! What this page will not draw, because the domain does not record it: a
//! "verified" share (there is no verification step); workflows in flight
//! (nothing links one task run to another); bays (a row is an agent, not a
//! slot -- `occupancy.rs`); and a daemon-down calendar tile (the store has
//! no event log, so a quiet day and a stopped daemon are the same shape of
//! nothing and must not be drawn as different facts). Cost stopped being one
//! of these with #117: a run's measured usage (`Run.usage`) now backs the
//! `cost` view tile and every registry metric's `usd`/`hours` figures
//! (#162) -- this comment used to say otherwise and was wrong by the time
//! #117 shipped. The production-year grid labels a day before this
//! instance's first recorded run "no record", not "before this factory
//! existed" -- the earliest run is a lower bound on how long the instance has
//! existed, not its birthday.
//!
//! Registry metric tiles and #150's richer view tiles (#162, phase 3) read
//! four more shared answers -- `/api/metrics`, `/api/occupancy`, `/api/
//! operations`, `/api/policy`, `/api/costs` -- each fetched at most once per
//! render cycle and only when the resolved layout actually names a tile
//! that needs it (`neededEndpoints`/`neededMetricIds`,
//! `dashboard-tiles-model.js`): the default layout names none of them, so it
//! starts no new tile request. `/api/metrics`, `/api/occupancy` and `/api/costs`
//! follow the dashboard's own window the same as `/api/production` does, so
//! a window change refetches all four (`reloadTileData`); `/api/policy` and
//! `/api/operations` do not, and only reload with the layout, on a scope
//! change. `/api/occupancy` carries no `scope` on the wire at all
//! (`Request::Occupancy`) -- every tile reading it narrows client-side with
//! `inScope`, the same as the occupancy chart itself.
//! Signposts are separate read-only observations: `/api/signposts` is read
//! once per load even on the default layout, never through Operations or
//! counted as waiting Inbox work.
//!
//! Recent events: dropped, on purpose, rather than duplicated. Activity
//! already is that view -- a tail of what this page has seen since it opened,
//! carrying its own "not an archive" banner (`activity.js`) -- and a second
//! copy on the dashboard would either repeat it one click away or have to
//! become the event store this prototype does not have. Needs-attention has its
//! own Inbox view, which reads the daemon's attention list (`GET
//! /api/operations`, `#106`): blocked runs with the agent's own words, runs
//! out of retries, lost standing agents, late and missed schedules -- the
//! same exceptions the L4 Operations tab shows per scope. Nothing here
//! decides what needs a person any more; `operations.rs` does, once. The
//! Inbox also reads the CRA Art. 14 reporting clock (`GET
//! /api/policy/clock`, `#157`/`#170` phase 2) alongside it, for its own
//! overdue and due-soon deadlines -- a second, independent read, shaped by
//! `clock-model.js`'s `inboxClockRows`, never folded into `operations.rs`'s
//! own attention list: a clock deadline is not one of the exception kinds
//! that endpoint speaks, and a failed clock read never blanks the rest of
//! the Inbox.

import { $, esc, api, state } from "./core.js";
import { inScope, routeHref, scopeLabel } from "./scopes.js";
import { ACTION_LABELS, actionRequest, fmtAge, inboxItems } from "./operations-model.js";
import { inboxClockRows } from "./clock-model.js";
import { signpostObservations } from "./signposts-model.js";
import { fetchDates } from "./dates.js";
import { renewalInboxRows, datesSummary } from "./dates-model.js";
import { taskUsageLine, costFigure } from "./usage-model.js";
import { openTask } from "./tasks.js";
import { openCreate } from "./task-form.js";
import { hasFailed } from "./task-model.js";
import { notStarted } from "./pending-model.js";
import { scrim, closeModal, dropModal } from "./modal.js";
import { packRows, resolveDashboard, isTileRenderable, tileKind, rowTemplate, rowIsAllSmall, SIZES } from "./dashboard-model.js";
import {
  neededEndpoints,
  neededMetricIds,
  metricTileView,
  agentHoursByScope,
  agentHoursByAgent,
  occupancyStripRows,
  complianceSummaryRows,
  topCostRows,
  windowDays,
} from "./dashboard-tiles-model.js";
import {
  VIEW_LABELS,
  buildCatalogue,
  searchCatalogue,
  seedTiles,
  addTile,
  removeTile,
  moveTileUp,
  moveTileDown,
  setTileSize,
  editorErrors,
  canReset,
  rootScopeName,
} from "./dashboard-editor-model.js";

/// The three presets the window selector offers. `bin` travels with every
/// request rather than being guessed from `minutes` server-side, so a caller
/// always gets exactly the granularity it draws (`production.rs`).
const WINDOWS = {
  day: { label: "Today", minutes: 24 * 60, bin: "hour" },
  d14: { label: "14 days", minutes: 14 * 24 * 60, bin: "day" },
  d90: { label: "90 days", minutes: 90 * 24 * 60, bin: "week" },
};
let windowKey = "d14";

/// `WINDOWS`' own keys, spelled the way `#182`'s `/api/metrics?window=`
/// expects them (`factory_core::metrics::MetricsWindow`) -- the dashboard's
/// three presets are exactly that enum's three values, just named
/// differently on this side of the wire.
const METRICS_WINDOW = { day: "day", d14: "14d", d90: "90d" };

/// The last answer from `/api/production`. Module-local, the way
/// `activity.js` keeps its own log: nothing outside this file draws history.
/// `undefined` means "not fetched yet", `null` means "the fetch failed" --
/// kept apart from "fetched, and genuinely nothing has ever finished" so the
/// three read as different cards rather than the same blank.
let production;

/// The last `/api/dashboard` answer (`#159`): `layoutTiles` is the
/// resolved tile list, or `null` for "nothing overrides anything, use the
/// built-in default" -- `resolveDashboard` (`dashboard-model.js`) is the
/// one place that decision is made, so `undefined` (not fetched yet) and a
/// failed fetch (set to `null` below) read exactly like the server's own
/// "no override" answer. `layoutSource` names the scope whose `dashboard:`
/// block won, or `null` for the same built-in-default case; shown, subtly,
/// in the bar (`renderDashSource`).
let layoutTiles;
let layoutSource = null;

/// The four new shared reads #162's tiles draw from -- each `undefined`
/// until its first fetch, `null` when that fetch failed, the same "three
/// states, never a manufactured empty answer" rule `production`/
/// `inboxReport` already keep. Only ever populated when `neededEndpoints`
/// says the current layout has a tile that needs it (`loadTileData`/
/// `reloadTileData` below) -- a layout without, say, a `cost` tile leaves
/// `tileCosts` exactly as it was, which is harmless: nothing reads it.
let tileMetrics; // last /api/metrics answer: { values, series, registry }
let tileOccupancy; // last /api/occupancy answer's `.occupancy`
let tilePolicy; // last /api/policy answer's `.report`
let tileCosts; // last /api/costs answer's `.report`
let tileDates;
let datesAsked = 0;

async function loadTileDates() {
  const scope = state.scope;
  const mine = ++datesAsked;
  const report = await fetchDates(scope);
  if (mine === datesAsked && scope === state.scope) tileDates = { scope, report };
}

function datesTile() {
  const report = tileDates?.scope === state.scope ? tileDates.report : null;
  const summary = datesSummary(report);
  return dcard("Important dates", summary.counts, `<p>Next expiry: ${esc(summary.next)}</p>${!report ? `<p class="sub">${esc(summary.text)}</p>` : ""}<a href="${esc(routeHref(state.scope, "dates"))}">Open important dates</a>`);
}

export async function loadDashboard() {
  // Production and the layout are independent reads, same as before #162;
  // only once the layout is resolved does anything know which of the new
  // shared reads (if any) the tiles on it actually need.
  await Promise.all([loadProduction(), loadLayout()]);
  const tiles = resolveDashboard(layoutTiles);
  await Promise.all([loadTileData(tiles), neededEndpoints(tiles).operations ? Promise.resolve() : loadSignposts()]);
  renderDashboard();
}

async function loadProduction() {
  const w = WINDOWS[windowKey];
  const params = new URLSearchParams({ minutes: String(w.minutes), bin: w.bin });
  if (state.scope) params.set("scope", state.scope);
  try {
    production = (await api(`/api/production?${params}`)).production;
  } catch {
    production = null;
  }
}

/// Fetches exactly the shared reads `tiles` needs (`neededEndpoints`/
/// `neededMetricIds`, `dashboard-tiles-model.js`) -- never more than one per
/// endpoint, and none at all for a layout (the default included) that names
/// no tile of that kind. Awaited alongside `loadProduction` from both
/// `loadDashboard` (a fresh layout, so every kind is worth checking) and
/// `reloadTileData` (the window changed; the layout did not).
async function loadTileData(tiles) {
  const needs = neededEndpoints(tiles);
  await Promise.all([
    needs.metrics ? loadTileMetrics(neededMetricIds(tiles)) : Promise.resolve(),
    needs.occupancy ? loadTileOccupancy() : Promise.resolve(),
    needs.operations ? loadInbox() : Promise.resolve(),
    needs.policy ? loadTilePolicy() : Promise.resolve(),
    needs.costs ? loadTileCosts() : Promise.resolve(),
    needs.dates ? loadTileDates() : Promise.resolve(),
  ]);
}

/// One `/api/metrics?ids=…&scope=&window=` request for every metric tile on
/// the page, scoped and windowed the same as `/api/production`. `ids` is
/// never empty here -- `loadTileData` only calls this when `neededEndpoints`
/// found a metric tile, and an empty `ids=` would ask the wire's own
/// default (every metric in the registry) instead of the few this layout
/// actually names.
async function loadTileMetrics(ids) {
  const params = new URLSearchParams({ ids: ids.join(",") });
  if (state.scope) params.set("scope", state.scope);
  params.set("window", METRICS_WINDOW[windowKey]);
  try {
    tileMetrics = await api(`/api/metrics?${params}`);
  } catch {
    tileMetrics = null;
  }
}

/// `/api/occupancy`, over the dashboard's own window -- the one read
/// `agent_hours_by_scope`/`_by_agent`/`occupancy_strip` all share. Unscoped
/// on the wire (`Request::Occupancy` carries no `scope`, the same as
/// `occupancy.js`'s own fetch): every tile that reads this filters rows
/// with `inScope` itself, client-side, the way the chart already does.
async function loadTileOccupancy() {
  try {
    tileOccupancy = (await api(`/api/occupancy?minutes=${WINDOWS[windowKey].minutes}`)).occupancy;
  } catch {
    tileOccupancy = null;
  }
}

/// `/api/policy?scope=`, for the `compliance` tile -- the same request
/// `policy.js`'s own board makes for the selected scope's subtree rollup.
async function loadTilePolicy() {
  const query = state.scope ? `?scope=${encodeURIComponent(state.scope)}` : "";
  try {
    tilePolicy = (await api(`/api/policy${query}`)).report;
  } catch {
    tilePolicy = null;
  }
}

/// `/api/costs?group_by=scope&scope=&from=&to=`, for the `cost` tile -- a
/// fixed grouping, no arbitrary parameter (#150 design §8), and `from`/`to`
/// set from the dashboard's own window rather than the endpoint's own
/// trailing-30-day default: #150 §4 asks for one scope and one window for
/// the whole page, and a cost figure is no more exempt from that than a
/// metric or an occupancy tile is.
async function loadTileCosts() {
  const now = Date.now();
  const params = new URLSearchParams({
    group_by: "scope",
    from: new Date(now - WINDOWS[windowKey].minutes * 60000).toISOString(),
    to: new Date(now).toISOString(),
  });
  if (state.scope) params.set("scope", state.scope);
  try {
    tileCosts = (await api(`/api/costs?${params}`)).report;
  } catch {
    tileCosts = null;
  }
}

/// The window selector's own reload: production and every window-dependent
/// tile read (`/api/metrics`, `/api/occupancy`, `/api/costs`) follow the
/// window; the layout and `/api/policy` do not, so switching windows never
/// re-fetches those -- `wireDashboard`'s window buttons call this, never
/// `loadDashboard`.
async function reloadTileData() {
  const tiles = resolveDashboard(layoutTiles);
  const needs = neededEndpoints(tiles);
  await Promise.all([
    loadProduction(),
    needs.metrics ? loadTileMetrics(neededMetricIds(tiles)) : Promise.resolve(),
    needs.occupancy ? loadTileOccupancy() : Promise.resolve(),
    needs.costs ? loadTileCosts() : Promise.resolve(),
  ]);
  renderDashboard();
}

async function loadLayout() {
  const params = new URLSearchParams();
  if (state.scope) params.set("scope", state.scope);
  const query = params.toString() ? `?${params}` : "";
  try {
    const answer = await api(`/api/dashboard${query}`);
    layoutTiles = answer.tiles;
    layoutSource = answer.source;
  } catch {
    layoutTiles = null;
    layoutSource = null;
  }
}

function renderDashSource() {
  const el = $("dash-source");
  if (!el) return;
  el.textContent = layoutSource ? `layout from ${layoutSource}` : "";
}

export function renderDashboard() {
  renderSignposts();
  renderDashSource();
  const el = $("dash");
  if (!el) return;
  // The rail decides how much of the instance this reads as. Every figure
  // below is a count of what is selected, so a scope's dashboard and the whole
  // instance's are the same page asked a narrower question.
  const tasks = [...state.tasks.values()].filter((t) => inScope(t.scope));
  const scopes = state.scopes.filter((s) => inScope(s.name));

  if (!scopes.length) {
    el.innerHTML = state.scope
      ? `<div class="empty">Nothing in ${esc(scopeLabel())}.</div>`
      : `<div class="empty">This instance has no configured scopes yet. Add a scope block to a
      directory's <code>.factory/config.yaml</code> and the dashboard, activity log, and site plan
      have something to draw -- right now there is nothing to show but this sentence.</div>`;
    return;
  }

  // `undefined` while the first fetch is still in flight; render the rest of
  // the page rather than block on it.
  const prod = production === undefined ? null : production;
  const everFinished = !!(prod && prod.earliest_run);

  // `#dash` is rebuilt from scratch below, which replaces `.calwrap` with a
  // fresh element at `scrollLeft: 0` -- the oldest 53 weeks, not the one
  // column anybody actually opens the card to look at. Read where the old
  // element was scrolled before it is thrown away: "at its right edge" (true
  // with no element yet, on the very first render) means the new one is put
  // at ITS right edge too, so the fix tracks today rather than a fixed pixel
  // count; anywhere else the reader chose is preserved as-is.
  const oldCal = el.querySelector(".calwrap");
  const calWasAtEdge = !oldCal || oldCal.scrollLeft >= oldCal.scrollWidth - oldCal.clientWidth - 2;
  const calScrollLeft = oldCal ? oldCal.scrollLeft : 0;

  el.innerHTML = renderTiles(resolveDashboard(layoutTiles), { tasks, scopes, prod, everFinished });

  wireCalToggle();
  // An `inbox` view tile (#162) draws its own `.inbox-item` rows inside
  // `#dash`, the same markup `#inbox`'s own `renderInbox` draws -- rewired
  // every redraw, same as the cal toggle, since `#dash`'s whole innerHTML
  // was just replaced above.
  wireInboxRows(el);

  // Restore scroll after the new `.calwrap` (if any -- a card with nothing
  // finished yet draws no grid at all) has real dimensions to measure. A
  // viewport wide enough to show every week has nothing to scroll past, so
  // the edge and 0 are the same place and this is a no-op there.
  const newCal = el.querySelector(".calwrap");
  if (newCal) {
    newCal.scrollLeft = calWasAtEdge ? newCal.scrollWidth - newCal.clientWidth : calScrollLeft;
  }
}

// ---------------------------------------------------------------- tiles (#163)

/// One view tile's whole rendered card, keyed by `dashboard-model.js`'s
/// view ids -- the map `renderTiles` dispatches through instead of the
/// page being one hard-coded template. `ctx` is the one shared read every
/// tile draws from (`tasks`, `scopes`, `prod`, `everFinished`): no tile
/// fetches its own history, so the throughput chart and the sparklines
/// under the KPIs can never disagree with each other, exactly as the
/// module doc comment above promises.
///
/// Each function returns one whole, self-contained card -- its own
/// `<section class="dcard">`/`<div class="kpis">`, heading included -- the
/// same fragment `renderDashboard`'s old inline template produced at that
/// spot; only `onTheLine` and `productionYear` already did, so `throughput`
/// and `byScope` gained the wrapper `renderDashboard` used to add around
/// them. Row grouping (which cards share a `.drow`) is decided once, by
/// `renderTiles` below, from `dashboard-model.js`'s `packRows` -- a tile
/// renderer never wraps itself in `.drow`.
const VIEW_RENDERERS = {
  kpis: (ctx) => `<div class="kpis">${kpis(ctx.tasks, ctx.scopes, ctx.prod, ctx.everFinished).join("")}</div>`,
  throughput: (ctx) => `<section class="dcard">
        <h3>Throughput<span class="r">${esc(WINDOWS[windowKey].label.toLowerCase())} · finished per ${esc(WINDOWS[windowKey].bin)} · reworked share at the base</span></h3>
        ${throughput(ctx.prod, ctx.everFinished)}
      </section>`,
  on_the_line: (ctx) => onTheLine(ctx.tasks, ctx.prod, ctx.everFinished),
  production_year: (ctx) => productionYear(ctx.prod, ctx.everFinished),
  by_scope: (ctx) => `<section class="dcard wide">
        <h3>By scope<span class="r">${ctx.scopes.length} scope${ctx.scopes.length === 1 ? "" : "s"}</span></h3>
        ${byScopeTable(ctx.tasks, ctx.scopes)}
      </section>`,
  // #162 (phase 3): the rest of #150's view catalogue, each off the one
  // shared read `loadTileData` fetched for it -- see that function's own
  // doc comment for which endpoint backs which id.
  agent_hours_by_scope: () => agentHoursTile("Agent hours by scope", agentHoursByScope(tileOccupancy)),
  agent_hours_by_agent: () => agentHoursTile("Agent hours by agent", agentHoursByAgent(tileOccupancy)),
  occupancy_strip: () => occupancyStripTile(),
  inbox: () => inboxTile(),
  compliance: () => complianceTile(),
  cost: () => costTile(),
  important_dates: () => datesTile(),
};

/// A tile `isTileRenderable` (`dashboard-model.js`) says this page cannot
/// draw -- a `view` id outside the vocabulary, which the closed enum on a
/// config's own `dashboard:` block should already have refused before this
/// page ever sees it -- gets a small, neutral card naming it instead of
/// nothing or a crash.
function placeholderTile(tile) {
  const name = (tile && (tile.metric || tile.view)) || "tile";
  return `<section class="dcard"><div class="empty">${esc(name)} — not drawn yet</div></section>`;
}

function renderTile(tile, ctx) {
  if (!isTileRenderable(tile)) return placeholderTile(tile);
  return tileKind(tile) === "metric" ? metricTile(tile) : VIEW_RENDERERS[tile.view](ctx);
}

/// Assembles the page from a tile list: pack tiles into 12-column rows
/// (`packRows`), then a row of two or more tiles shares one `.drow` and a
/// row of one tile renders bare -- no `.drow` around a lone card, exactly as
/// before #162. `.drow` is a span-aware grid now (`rowTemplate`,
/// `dashboard-model.js`): fr-weighted per tile by its own size, set as the
/// `--cols` custom property `app.css`'s `.drow` rule reads. For the one
/// multi-tile row `DEFAULT_DASHBOARD` ever produces (`[l, m]`, 8 and 4 of
/// 12), `rowTemplate` returns `null` -- nothing to override, so that row's
/// markup is exactly what it always was and `app.css`'s own `2fr / 1fr`
/// fallback draws it, pixel for pixel. `rowIsAllSmall` marks a row of only
/// `s` tiles `drow-s`, so it goes two-up rather than one-per-line at the
/// narrow breakpoint (`app.css`) -- `DEFAULT_DASHBOARD` has no `s` tile, so
/// this never applies to it either.
function renderTiles(tiles, ctx) {
  return packRows(tiles)
    .map((row) => {
      if (row.length <= 1) return renderTile(row[0], ctx);
      const template = rowTemplate(row);
      const cls = rowIsAllSmall(row) ? "drow drow-s" : "drow";
      const style = template ? ` style="--cols:${esc(template)}"` : "";
      return `<div class="${cls}"${style}>${row.map((t) => renderTile(t, ctx)).join("")}</div>`;
    })
    .join("");
}

// -------------------------------------------------------------------- KPIs

function kpi(k, v, sub, opts) {
  const { unit, tone, spark } = opts || {};
  return `<div class="kpi"><span class="k">${esc(k)}</span>
    <span class="v">${esc(v)}${unit ? `<small>${esc(unit)}</small>` : ""}</span>
    <span class="d${tone ? ` ${tone}` : ""}">${esc(sub)}</span>${spark || ""}</div>`;
}

function sum(buckets, field) {
  return buckets.reduce((s, b) => s + b[field], 0);
}

/// A small trend line under a KPI. Deliberately not drawn for a KPI with no
/// historical series of its own -- "in flight" and "due" are what is true
/// this instant, and the daemon keeps no series of what that number was an
/// hour ago, so a sparkline there would have to be invented.
function sparkline(values, colour) {
  if (!values || values.length < 2) return "";
  const w = 100, h = 26;
  const max = Math.max(...values, 0);
  const min = Math.min(...values, 0);
  const span = max - min || 1;
  const step = w / (values.length - 1);
  const pts = values.map((v, i) => `${(i * step).toFixed(1)},${(h - ((v - min) / span) * h).toFixed(1)}`).join(" ");
  return `<svg viewBox="0 0 ${w} ${h}" preserveAspectRatio="none"><polyline points="${pts}" fill="none" stroke="var(--${colour})" stroke-width="1.6"/></svg>`;
}

function kpis(tasks, scopes, prod, everFinished) {
  const holding = tasks.filter((t) => t.status === "running" || t.status === "dispatching");
  const blocked = tasks.filter((t) => t.status === "blocked");
  const waiting = notStarted(tasks, Date.now());
  const agentCount = scopes.reduce((n, s) => n + s.agents.length, 0);
  const standingUp = scopes.reduce(
    (n, s) => n + s.agents.filter((a) => a.state === "ready" || a.state === "starting").length,
    0
  );

  const buckets = prod ? prod.buckets : [];
  const finished = sum(buckets, "finished");
  const scrapped = sum(buckets, "scrapped");
  const reworked = sum(buckets, "reworked");
  const winLabel = WINDOWS[windowKey].label.toLowerCase();
  // A flat line at zero would read as "a stable trend of zero", which is not
  // the same fact as "no history exists for this window" -- so nothing is
  // drawn at all until at least one run has ever finished.
  const finishedSpark = everFinished ? sparkline(buckets.map((b) => b.finished), "run") : "";
  const scrapSpark = everFinished ? sparkline(buckets.map((b) => (b.finished ? (b.scrapped / b.finished) * 100 : 0)), "fault") : "";
  const reworkSpark = everFinished ? sparkline(buckets.map((b) => (b.finished ? (b.reworked / b.finished) * 100 : 0)), "wait") : "";

  return [
    kpi("In flight", holding.length + blocked.length, `${holding.length} holding an agent · ${blocked.length} blocked`),
    // Due, not "queued": there is no queue behind a pending task (`#124`).
    // This is the count Operations calls `flow.queue_depth` -- a scheduled
    // slot that has come and not been dispatched yet -- and the manual tasks
    // that nothing will ever start on its own are named beside it, not
    // added into it.
    kpi("Due", waiting.due.length, notStartedSub(waiting)),
    kpi("Finished", finished, `${winLabel}${scrapped ? ` · ${scrapped} scrapped` : ""}`, { spark: finishedSpark }),
    kpi("Scrap rate", finished ? round1(scrapped / finished * 100) : "—", finished ? `of finished, ${winLabel}` : "nothing finished yet", { unit: finished ? "%" : "", spark: scrapSpark }),
    kpi("Reworked", finished ? round1(reworked / finished * 100) : "—", finished ? `of finished, ${winLabel}` : "nothing finished yet", { unit: finished ? "%" : "", spark: reworkSpark }),
    kpi("Scopes · agents", `${scopes.length} · ${agentCount}`, `${standingUp} standing agent${standingUp === 1 ? "" : "s"} up`),
  ];
}

function notStartedSub(w) {
  const parts = [`${w.manual.length} manual, not run`];
  if (w.later.length) parts.push(`${w.later.length} scheduled later`);
  if (w.waiting.length) parts.push(`${w.waiting.length} waiting for a slot`);
  return parts.join(" · ");
}

function round1(n) {
  return Math.round(n * 10) / 10;
}

// -------------------------------------------------------------- throughput

/// `day.month.year`, the same numeric style `occupancy.js`'s `clockLabel`
/// uses for its coarse ticks -- never `toLocaleDateString`, which reads the
/// browser's own locale and would draw one date on this page in a different
/// language from the rest of it.
function dmy(iso) {
  const d = new Date(iso);
  return `${d.getUTCDate()}.${d.getUTCMonth() + 1}.${d.getUTCFullYear()}`;
}

function bucketLabel(b, bin) {
  const d = new Date(b.from);
  if (bin === "hour") return `${String(d.getHours()).padStart(2, "0")}:00`;
  return `${d.getDate()}.${d.getMonth() + 1}.`;
}

function partialWords(b) {
  const ms = new Date(b.to) - new Date(b.from);
  const hrs = ms / 3600000;
  if (hrs < 1) return `${Math.round(ms / 60000)}m old`;
  if (hrs < 48) return `${hrs.toFixed(1)}h old`;
  return `${Math.round(hrs / 24)}d old`;
}

function throughput(prod, everFinished) {
  if (!prod) return `<div class="err">Run history is not available right now.</div>`;
  if (!everFinished) return `<div class="empty">Nothing has finished in this instance yet.</div>`;

  const buckets = prod.buckets;
  const W = 600, H = 160, padL = 4, padR = 4, padT = 6, padB = 16;
  const innerW = W - padL - padR;
  const innerH = H - padT - padB;
  const maxV = Math.max(1, ...buckets.map((b) => b.finished));
  const n = buckets.length;
  const gap = n > 40 ? 1 : 3;
  const bw = Math.max(1, (innerW - gap * (n - 1)) / n);
  // The mean excludes a bucket still filling -- it did not get the same
  // amount of time as the others to accumulate finished runs, so counting it
  // would drag the line down and read as a slowdown that has not happened.
  const full = buckets.filter((b) => !b.partial);
  const mean = full.length ? sum(full, "finished") / full.length : 0;
  const y = (v) => padT + innerH - (v / maxV) * innerH;

  let bars = "";
  buckets.forEach((b, i) => {
    const x = padL + i * (bw + gap);
    const hTotal = (b.finished / maxV) * innerH;
    const hRework = b.finished ? (b.reworked / b.finished) * hTotal : 0;
    const yTop = padT + innerH - hTotal;
    const yReworkTop = padT + innerH - hRework;
    const title = `${bucketLabel(b, prod.bin)} — ${b.finished} finished${b.reworked ? `, ${b.reworked} reworked` : ""}${b.scrapped ? `, ${b.scrapped} scrapped` : ""}${b.partial ? ` · still filling, ${partialWords(b)}` : ""}`;
    bars += `<g${b.partial ? ' class="bc-partial"' : ""}><title>${esc(title)}</title>`;
    if (hTotal > 0.2) {
      bars += `<rect x="${x.toFixed(1)}" y="${yTop.toFixed(1)}" width="${bw.toFixed(1)}" height="${hTotal.toFixed(1)}" class="bc-run"/>`;
      if (hRework > 0.2) bars += `<rect x="${x.toFixed(1)}" y="${yReworkTop.toFixed(1)}" width="${bw.toFixed(1)}" height="${hRework.toFixed(1)}" class="bc-rework"/>`;
    }
    bars += `</g>`;
  });

  const meanLine = full.length ? `<line x1="${padL}" y1="${y(mean).toFixed(1)}" x2="${W - padR}" y2="${y(mean).toFixed(1)}" class="bc-mean"/>` : "";
  const last = buckets[buckets.length - 1];
  const partialNote = last.partial
    ? `<div class="dnote">The last bar covers ${esc(partialWords(last))} of a full ${esc(prod.bin)} -- it has not finished collecting yet, so a short bar there is not a slow period.</div>`
    : "";

  return `<svg class="bigchart" viewBox="0 0 ${W} ${H}" preserveAspectRatio="none" role="img"
      aria-label="Runs finished per ${esc(prod.bin)}, reworked share at the base of each bar.">${bars}${meanLine}</svg>
    <div class="chleg"><span><i class="l-run"></i>finished</span><span><i class="l-rework"></i>reworked</span>
      ${full.length ? `<span style="margin-left:auto">mean ${round1(mean)} / ${esc(prod.bin)}</span>` : ""}</div>
    ${partialNote}`;
}

// -------------------------------------------------------------- on the line

function stnGroup(title) {
  return `<div class="stngrp">${esc(title)}</div>`;
}

function stnRow(label, count, total, colour) {
  const pct = total ? (count / total) * 100 : 0;
  return `<div class="stn"><span class="nm">${esc(label)}</span>
    <span class="track"><i style="width:${pct.toFixed(1)}%;background:var(--${colour})"></i></span>
    <span class="c">${count}</span></div>`;
}

/// "On the line": the same live task figures the KPI row counts, broken into
/// where the work is, plus what left the line today. Groups one and two are
/// not a second tally -- they are the In flight and Due KPIs, split out
/// and explained, so the two can never quietly drift apart.
function onTheLine(tasks, prod, everFinished) {
  const running = tasks.filter((t) => t.status === "running" || t.status === "dispatching");
  const blocked = tasks.filter((t) => t.status === "blocked");
  const waiting = notStarted(tasks, Date.now());
  const pending = waiting.due.length + waiting.later.length + waiting.manual.length + waiting.waiting.length;
  const holding = running.length + blocked.length;

  const today = prod && prod.daily.length ? prod.daily[prod.daily.length - 1] : null;
  const shipped = today ? today.finished - today.scrapped : 0;
  const scrappedToday = today ? today.scrapped : 0;

  let todayRows;
  let todayNote;
  if (!prod) {
    todayRows = `<div class="empty">Run history is not available right now.</div>`;
    todayNote = "";
  } else if (!everFinished) {
    todayRows = `<div class="empty">Nothing has finished in this instance yet.</div>`;
    todayNote = "";
  } else {
    todayRows =
      stnRow("Shipped", shipped, today.finished, "run") +
      stnRow("Scrapped", scrappedToday, today.finished, "fault");
    todayNote = today.partial
      ? ` Today is ${esc(partialWords(today))} -- more may still land before it ends.`
      : "";
  }

  return `<section class="dcard">
    <h3>On the line<span class="r">right now</span></h3>
    ${stnGroup("Holding an agent")}
    ${stnRow("Running", running.length, holding || 1, "run")}
    ${stnRow("Blocked", blocked.length, holding || 1, "wait")}
    ${stnGroup("Not started · no run yet")}
    ${stnRow("Due", waiting.due.length, pending || 1, "idle")}
    ${stnRow("Scheduled later", waiting.later.length, pending || 1, "idle")}
    ${stnRow("Manual, not run", waiting.manual.length, pending || 1, "idle")}
    ${stnRow("Waiting for a slot", waiting.waiting.length, pending || 1, "idle")}
    ${stnGroup(`Left the line today${today ? ` · against ${today.finished} finished` : ""}`)}
    ${todayRows}
    <p class="dnote">${holding} above hold an agent -- the same count the In flight KPI shows. ${pending} more
      ${pending === 1 ? "has" : "have"} not started: ${waiting.due.length} due, which the scheduler will fire -- the Due KPI
      and the Operations queue. Nothing starts a manual task on its own; run it with
      <code>factory task run &lt;id&gt;</code> or its Run button.${todayNote}</p>
  </section>`;
}

// --------------------------------------------------------- production year

const DOW = ["Mon", "", "Wed", "", "Fri", "", "Sun"];

function dateOf(b) {
  return new Date(b.from).toISOString().slice(0, 10);
}

function buildWeeks(daily) {
  if (!daily.length) return [];
  const first = new Date(daily[0].from);
  const lead = (first.getUTCDay() + 6) % 7; // 0 = Monday
  const cells = new Array(lead).fill(null).concat(daily);
  const weeks = [];
  for (let i = 0; i < cells.length; i += 7) weeks.push(cells.slice(i, i + 7));
  const last = weeks[weeks.length - 1];
  while (last.length < 7) last.push(null);
  return weeks;
}

// Fixed English abbreviations, not `toLocaleString` -- the rest of the page
// (`occupancy.js`'s `clockLabel`) never asks the browser's locale either, so
// a viewer whose OS is set to another language does not get one calendar in
// a different language from everything around it.
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

function monthLabels(weeks) {
  let seen = null;
  return weeks.map((week) => {
    const real = week.find((d) => d);
    if (!real) return "";
    const m = dateOf(real).slice(0, 7);
    if (m === seen) return "";
    seen = m;
    return MONTHS[new Date(real.from).getUTCMonth()];
  });
}

let calMode = "runs";

function calTile(day, maxFinished, earliestRun) {
  if (!day) return `<i class="future"></i>`;
  const date = dateOf(day);
  if (!earliestRun || date < earliestRun.slice(0, 10)) {
    return `<i class="no-record" title="${esc(date)} — no record"></i>`;
  }
  const scrapShare = day.finished ? (day.scrapped / day.finished) * 100 : 0;
  const bad = scrapShare > 15;
  // Runs mode scales green by volume; scrap-share mode scales red by the
  // scrap percentage -- two different scales, so a busy-but-clean day and a
  // slow-but-scrappy one never share a colour by coincidence.
  const cls = calMode === "runs"
    ? `l${day.finished === 0 ? 0 : Math.min(4, 1 + Math.floor((day.finished / (maxFinished || 1)) * 3))}`
    : `s${day.finished === 0 ? 0 : Math.min(4, 1 + Math.floor((scrapShare / 100) * 3))}`;
  const title = `${date} — ${day.finished} finished${day.reworked ? `, ${day.reworked} reworked` : ""}${day.finished ? `, ${round1(scrapShare)}% scrapped` : ""}`;
  return `<i class="${cls}${bad ? " bad" : ""}" title="${esc(title)}"></i>`;
}

function productionYear(prod, everFinished) {
  if (!prod) {
    return `<section class="dcard"><h3>The production year<span class="r">one tile per day</span></h3>
      <div class="err">Run history is not available right now.</div></section>`;
  }
  if (!everFinished) {
    return `<section class="dcard"><h3>The production year<span class="r">one tile per day</span></h3>
      <div class="empty">Nothing has finished in this instance yet -- every tile would read "no record".</div></section>`;
  }
  const daily = prod.daily;
  const weeks = buildWeeks(daily);
  const maxFinished = Math.max(1, ...daily.map((d) => d.finished));
  const months = monthLabels(weeks);
  const earliest = prod.earliest_run;

  const grid = weeks
    .map((week) => `<div class="cw">${week.map((d) => calTile(d, maxFinished, earliest)).join("")}</div>`)
    .join("");

  // 371 days is 53 weeks exactly; the grid pads a partial leading week so
  // every column lines up on a Monday, which can add a 54th column of mostly
  // blank cells. Said as "53 weeks" regardless -- that padding is chrome for
  // alignment, not a 54th week of data.
  return `<section class="dcard">
    <h3>The production year<span class="r">one tile per day · 53 weeks</span>
      <span class="seg" id="cal-seg" style="margin-left:12px">
        <button data-c="runs" class="${calMode === "runs" ? "on" : ""}">Runs</button>
        <button data-c="scrap" class="${calMode === "scrap" ? "on" : ""}">Scrap share</button>
      </span>
    </h3>
    <div class="calwrap">
      <div class="calmon">${months.map((m) => `<span>${esc(m)}</span>`).join("")}</div>
      <div class="calbody">
        <div class="caldow">${DOW.map((d) => `<span>${esc(d)}</span>`).join("")}</div>
        <div class="calgrid">${grid}</div>
      </div>
      <div class="calleg">
        <span class="k"><i class="no-record"></i>no record</span>
        <span class="k"><i class="l3 bad"></i>&gt; 15% scrapped</span>
      </div>
    </div>
    <p class="calfoot">Colour is ${calMode === "runs" ? "how many runs finished" : "what share was scrapped"} that day. A
      hatch marks any day where more than 15% of what finished was scrapped -- a busy day producing rework can look
      like a good day on volume alone, and the hatch is what tells the two apart.
      ${earliest ? `The earliest recorded run is ${dmy(earliest)}; that is a lower bound on how long this instance has existed, not its birthday, so days before it read "no record".` : ""}</p>
  </section>`;
}

// ------------------------------------------------------------------ by scope

function byScopeTable(tasks, scopes) {
  if (!scopes.length) return `<div class="empty">No scopes declared.</div>`;
  const rows = scopes.map((s) => {
    const ts = tasks.filter((t) => t.scope === s.name);
    // A task blocked by a failure is not active work: it counts as failed.
    const active = ts.filter((t) => (t.status === "running" || t.status === "dispatching" || t.status === "verifying" || t.status === "blocked") && !hasFailed(t)).length;
    // Blocked by a failure since #122; a legacy `failed` row counts too.
    const failed = ts.filter(hasFailed).length;
    return `<tr>
      <td><div class="title">${esc(s.name)}</div><div class="sub">${esc(s.path)}</div></td>
      <td class="sub">${s.agents.length}</td>
      <td class="sub">${active}</td>
      <td class="sub">${ts.length}</td>
      <td class="sub">${failed}</td>
    </tr>`;
  }).join("");
  return `<table>
    <thead><tr><th>Scope</th><th>Agents</th><th>Active</th><th>Tasks</th><th>Failed</th></tr></thead>
    <tbody>${rows}</tbody>
  </table>`;
}

// --------------------------------------------------------- view tiles (#162)

/// The small card every view tile below shares: a heading, an optional
/// right-aligned qualifier (`.dcard h3 .r`, the same slot Throughput's own
/// window label sits in), and a body. Kept as one function so a tile never
/// has to restate `<section class="dcard">`/`<h3>` by hand.
function dcard(title, qualifier, body) {
  const right = qualifier ? '<span class="r">' + esc(qualifier) + "</span>" : "";
  return `<section class="dcard"><h3>${esc(title)}${right}</h3>${body}</section>`;
}

/// " · trailing N days" for a window the answer itself reported, or "" when
/// it reported none -- the one place a tile qualifier spells a day count.
function trailingDays(days) {
  if (days === null) return "";
  return ` · trailing ${days} ${days === 1 ? "day" : "days"}`;
}

/// One `.stn` bar row (`app.css`, the same markup `onTheLine`'s `stnRow`
/// draws) -- reused here rather than reimplemented, since a labelled bar
/// with a trailing figure is exactly what every new view tile below needs.
/// `note`, if given, is raw HTML appended after the figure -- the blocked-
/// hours aside, or nothing. `pct` of `null` draws the empty groove with no
/// fill at all, never a `0%`-wide one: a real zero and "there is nothing
/// here to compare" are different facts, and a bar filled to nothing reads
/// as the former (`costTile`'s own reason for passing `null`).
function barRow(label, pct, figure, note) {
  const track = pct === null
    ? `<span class="track"></span>`
    : `<span class="track"><i style="width:${Math.max(0, Math.min(100, pct)).toFixed(1)}%;background:var(--run)"></i></span>`;
  return `<div class="stn"><span class="nm">${esc(label)}</span>
    ${track}
    <span class="c">${esc(figure)}${note || ""}</span></div>`;
}

/// A registry `metric` tile -- reuses `.kpi` (`app.css`), the same card
/// `kpis()` already draws six of, so a metric split out on its own still
/// looks like the figure it came from. `metricTileView`
/// (`dashboard-tiles-model.js`) does every bit of the shaping (unit,
/// unavailable reason, better-direction glyph, the instance-wide qualifier,
/// its own series); this only turns that into markup, the same split every
/// other tile below keeps.
function metricTile(tile) {
  if (tileMetrics === undefined) {
    return `<div class="kpi"><span class="k">${esc(tile.metric)}</span><span class="d">loading…</span></div>`;
  }
  const view = metricTileView(tile.metric, tileMetrics);
  const subParts = [];
  if (!view.unavailable) {
    if (view.better) subParts.push(`${view.better} better`);
    if (view.coverage) subParts.push(view.coverage);
  }
  const sub = view.unavailable ? view.reason : subParts.join(" · ");
  const spark = !view.unavailable && view.points && view.points.length >= 2 ? sparkline(view.points, "run") : "";
  return `<div class="kpi"><span class="k">${esc(view.title)}</span>
    <span class="v">${esc(view.unavailable ? "—" : view.valueText)}</span>
    <span class="d">${esc(sub)}</span>${spark}</div>`;
}

/// The window a `tileOccupancy` answer actually covers, as a qualifier --
/// `from` to `now`, never `to` (which overshoots a quarter-window ahead to
/// leave room for a scheduled run's own preview, `occupancy.js`'s own
/// `liveView`). Said plainly rather than assumed from the dashboard's own
/// window key: `/api/occupancy` clamps the window it serves (`occupancy.rs`'s
/// `MIN_MINUTES`/`MAX_MINUTES`), so a request outside that range answers
/// something other than what was asked, and a tile that named the window it
/// asked for rather than the one it got would disagree with an `agent_hours`
/// metric tile sitting beside it.
function occupancyWindowNote(occ) {
  const days = windowDays(occ.from, occ.now);
  return "from run blocks" + trailingDays(days);
}

/// `agent_hours_by_scope`/`agent_hours_by_agent`: one bar per row from
/// `agentHoursByScope`/`agentHoursByAgent` (`dashboard-tiles-model.js`, off
/// the one shared `/api/occupancy` read, `tileOccupancy`) -- busy hours as
/// the bar, blocked hours named beside the figure, never subtracted from
/// it, the same rule the occupancy chart itself keeps for the same numbers.
/// `row.label` is already the right display name either way: a scope's own
/// name from `agentHoursByScope`, or `agentHoursByAgent`'s own disambiguated
/// `"<agent> · <scope>"` for a name that is not unique on this tile (`shell`
/// turns up once per scope) -- this never has to know which. Sorted busiest
/// first already, capped here at ten rows so the card stays a card.
function agentHoursTile(title, rows) {
  if (tileOccupancy === undefined) return dcard(title, "from run blocks", `<div class="empty">loading…</div>`);
  if (tileOccupancy === null) return dcard(title, "", `<div class="err">Occupancy is not available right now.</div>`);
  const qualifier = occupancyWindowNote(tileOccupancy);
  if (!rows.length) return dcard(title, qualifier, `<div class="empty">No agents in this window.</div>`);
  const max = Math.max(1, ...rows.map((r) => r.busyHours));
  const body = rows.slice(0, 10).map((r) => {
    const note = r.blockedHours > 0.05
      ? ` <span class="occ-wait" title="blocked time is part of the hours worked, shown beside them, never taken off">· ${r.blockedHours.toFixed(1)}h blocked</span>`
      : "";
    return barRow(r.label, (r.busyHours / max) * 100, `${r.busyHours.toFixed(1)}h`, note);
  }).join("");
  return dcard(title, qualifier, body);
}

/// `occupancy_strip`: a compact per-agent utilisation strip -- one bar row
/// per agent, `occupancyStripRows`' own `pct` of the window each covered,
/// no blocks or spans underneath it the way the full occupancy chart draws.
/// `r.label` is `occupancyStripRows`' own disambiguated name (`"<agent> ·
/// <scope>"` when the bare name is not unique on this tile -- see
/// `agentHoursTile`'s own note). Capped at fourteen rows, busiest first.
function occupancyStripTile() {
  if (tileOccupancy === undefined) return dcard("Occupancy strip", "", `<div class="empty">loading…</div>`);
  if (tileOccupancy === null) return dcard("Occupancy strip", "", `<div class="err">Occupancy is not available right now.</div>`);
  const rows = occupancyStripRows(tileOccupancy, Date.now());
  const qualifier = `share busy · ${occupancyWindowNote(tileOccupancy)}`;
  if (!rows.length) return dcard("Occupancy strip", qualifier, `<div class="empty">No agents in this window.</div>`);
  const body = rows.slice(0, 14).map((r) => barRow(r.label, r.pct, `${r.pct}%`)).join("");
  return dcard("Occupancy strip", qualifier, body);
}

/// `compliance`: per-framework met/open from `/api/policy` (`tilePolicy`),
/// shaped by `complianceSummaryRows` (`dashboard-tiles-model.js`) the same
/// way `compliance_value`/`open_controls_value` split it server-side. The
/// qualifier names the same subtree `policy.js`'s own board note does.
function complianceTile() {
  if (tilePolicy === undefined) return dcard("Compliance", "", `<div class="empty">loading…</div>`);
  if (tilePolicy === null) return dcard("Compliance", "", `<div class="err">Policy is not available right now.</div>`);
  const rows = complianceSummaryRows(tilePolicy);
  if (!rows.length) return dcard("Compliance", "", `<div class="empty">No framework catalogues loaded.</div>`);
  const body = rows.map((r) => barRow(r.title, r.counted ? (r.met / r.counted) * 100 : 0, `${r.met}/${r.counted || 0}`)).join("");
  const qualifier = state.scope ? `${state.scope} and below` : "whole instance's rollup";
  return dcard("Compliance", qualifier, body);
}

/// `cost`: `/api/costs?group_by=scope&from=&to=` (`tileCosts`), `from`/`to`
/// set from the dashboard's own window (`loadTileCosts`) -- the total line
/// reuses `usage-model.js`'s own `taskUsageLine` (a `CostReport.total` is
/// shaped exactly like the task-usage total it was written for), then the
/// most-expensive rows (`topCostRows`), each through `costFigure` (also
/// `usage-model.js`): a row whose runs are all usage-unknown reads
/// "unknown" with no bar at all, never a measured-looking `$0.00`; one with
/// some unmeasured, uncosted or still-partial runs reads a `≥`-prefixed
/// lower bound. The qualifier reads the answer's own `from`/`to`, not the
/// dashboard's window key: an honest figure for whatever span the daemon
/// actually answered, the same reason `occupancyWindowNote` reads
/// `tileOccupancy`'s own bounds.
function costTile() {
  if (tileCosts === undefined) return dcard("Cost", "", `<div class="empty">loading…</div>`);
  if (tileCosts === null) return dcard("Cost", "", `<div class="err">Costs are not available right now.</div>`);
  const line = taskUsageLine(tileCosts.total) || "No runs in this window.";
  const total = tileCosts.total.cost_usd || 0;
  const rows = topCostRows(tileCosts, 5)
    .map((r) => {
      const { text, hasCost } = costFigure(r);
      const pct = hasCost && total ? (r.cost_usd / total) * 100 : null;
      return barRow(r.label || r.key, pct, text);
    })
    .join("");
  const days = windowDays(tileCosts.from, tileCosts.to);
  const qualifier = "API-equivalent USD · by scope" + trailingDays(days);
  return dcard("Cost", qualifier, `<p class="dnote">${esc(line)}</p>${rows}`);
}

// --------------------------------------------------------------------- inbox

/// The last `/api/operations` answer the Inbox drew from. `undefined` until
/// the first fetch, `null` when it failed -- kept apart so "loading", "not
/// available" and "nothing waiting" read as three different things.
let inboxReport;
/// The last `/api/policy/clock` answer (`#157`/`#170` phase 2), fetched
/// alongside it. `null` covers both "not fetched" and "the read failed" --
/// unlike `inboxReport`, its own failure never blanks the Inbox: it is an
/// addition to the daemon's attention queue, not the queue itself, and
/// `inboxClockRows` already returns `[]` for a `null` clock (`clock-model.js`).
let clockReport;
let renewalReport;
let signpostReport;
let signpostsAsked = 0;
let inboxAsked = 0;
let inboxReceivedAt = 0; // when it arrived, on this browser's clock -- ages grow from there

async function fetchOperationsReport() {
  try {
    return (await api("/api/operations")).report;
  } catch {
    return null;
  }
}

async function loadSignposts() {
  const mine = ++signpostsAsked;
  let fact;
  try {
    fact = (await api("/api/signposts")).fact;
    if (!fact || !Array.isArray(fact.triggered)) fact = null;
  } catch { fact = null; }
  if (mine !== signpostsAsked) return;
  signpostReport = fact;
  renderSignposts();
}

function renderSignposts() {
  const rows = signpostObservations(signpostReport, routeHref(null, "scenarios"));
  for (const id of ["dash-signposts", "inbox-signposts"]) {
    const host = $(id);
    if (!host) continue;
    host.hidden = signpostReport !== null && rows.length === 0;
    host.innerHTML = signpostReport === null
      ? '<p class="sub">Scenario observations are unavailable; process attention is separate.</p>'
      : '<h3>Scenario observations</h3><p class="sub">Read-only signposts; no automatic action.</p><div class="inbox-list">' + rows.map(inboxItemRow).join("") + "</div>";
    wireInboxRows(host);
  }
}

async function fetchClock() {
  try {
    return (await api("/api/policy/clock")).clock;
  } catch {
    return null;
  }
}

/// A report clock item's own title: its task's (a confirmed security report
/// is a task, `#170`; `state.tasks` already keeps every task Factory
/// knows about) or, until that task is loaded, its bare id.
function clockReportTitle(taskId) {
  const t = state.tasks.get(taskId);
  return t?.title || taskId;
}

/// A finding has no task of its own to open -- Dependencies is where its
/// evidence lives.
function clockFindingHref(scope) {
  return routeHref(scope, "dependencies");
}

/// The Inbox's whole list: the daemon's own attention queue (`inboxItems`,
/// `operations-model.js`) plus the reporting clock's own overdue and
/// due-soon deadlines (`inboxClockRows`, `clock-model.js`) -- read from a
/// different endpoint and shaped by a different pure function on purpose:
/// a clock deadline is not an `ExceptionKind` `operations.rs` ever emits,
/// so it never goes through `attentionRows`, which mirrors that closed
/// vocabulary exactly. Concatenated, not re-sorted against each other --
/// each block is already ordered within itself, and a missed legal
/// deadline leads.
function inboxRows(elapsedS) {
  return [...inboxClockRows(clockReport, elapsedS, clockReportTitle, clockFindingHref), ...renewalInboxRows(renewalReport), ...inboxItems(inboxReport, elapsedS)];
}

/// The Inbox is the daemon's attention list (`#106`), every scope, minus
/// the observations: the same exceptions the Operations tab shows per scope
/// with flow context around them. It used to be derived here from
/// `state.tasks` -- blocked, failed, cancelled, overdue -- which counted a
/// failure a retry was about to fix and missed a lost standing agent; the
/// daemon now decides once what needs a person, and both pages read it.
export async function loadInbox() {
  // Two refetches can overlap; only the newest one's answer is drawn, or a
  // slow old read could land last and bring back what was just resolved.
  const mine = ++inboxAsked;
  const [ops, clock, renewals] = await Promise.all([fetchOperationsReport(), fetchClock(), fetchDates(), loadSignposts()]);
  if (mine !== inboxAsked) return;
  inboxReport = ops;
  clockReport = clock;
  renewalReport = renewals;
  inboxReceivedAt = Date.now();
  renderInbox();
}

/// One attention item's row -- shared by the Inbox nav view (`renderInbox`,
/// the whole list) and the dashboard's own `inbox` tile (`inboxTile`, top
/// few), so there is one markup for it, not two that could drift apart.
function inboxItemRow(it) {
  // `it.href`: a row with no task of its own but somewhere to go anyway --
  // today only a reporting-clock finding, pointed at Dependencies
  // (`clockFindingHref`, `inboxRows`). `liveness_lost` is the older,
  // special-cased fallback for the same shape of thing.
  const fallbackHref = it.href || (it.kind === "liveness_lost" ? routeHref(null, "roster") : "");
  const href = it.task_id ? "" : fallbackHref;
  // The reason is the agent's own words when it gave any -- a blocked
  // run's question, a failure's last error -- and the daemon's otherwise.
  return `<div class="inbox-item"${it.task_id ? ` data-task="${esc(it.task_id)}"` : ""}${it.run_id ? ` data-run="${esc(it.run_id)}"` : ""}${href ? ` data-href="${esc(href)}"` : ""}>
      <span class="it-dot" style="background:var(--${it.tone})"></span>
      <span class="it-t"><b>${esc(it.title || it.agent || "")}</b> — ${esc(it.label)}${it.suspicion ? ` <span class="ops-suspect">suspicion</span>` : ""}
        <span class="sub">${esc(it.reason)}${it.scope ? ` · ${esc(it.scope)}` : ""}</span>
      </span>
      <span class="it-age">${esc(fmtAge(it.age))}</span>
      ${(it.actions || []).filter(a => ["approve", "reject", "accept_rework"].includes(a)).map(a => `<button type="button" class="btn inbox-action" data-action="${esc(a)}">${esc(ACTION_LABELS[a] || a)}</button>`).join("")}
    </div>`;
}

/// Wires every `.inbox-item` under `host` the same way, whichever of the two
/// places drew them.
function wireInboxRows(host) {
  for (const row of host.querySelectorAll(".inbox-item")) {
    row.onclick = () => {
      if (row.dataset.task) openTask(row.dataset.task, row.dataset.run);
      else if (row.dataset.href) location.hash = row.dataset.href;
    };
  }
  for (const button of host.querySelectorAll(".inbox-action")) {
    button.onclick = async (event) => {
      event.stopPropagation();
      const row = button.closest(".inbox-item");
      const items = inboxItems(inboxReport, (Date.now() - inboxReceivedAt) / 1000);
      const item = items.find(it => it.run_id === row?.dataset.run);
      if (!item) return;
      const action = button.dataset.action;
      const reason = ["approve", "reject"].includes(action)
        ? window.prompt(`${ACTION_LABELS[action]} reason:`)
        : "";
      if (["approve", "reject"].includes(action) && !reason?.trim()) return;
      const req = actionRequest(action, item, { reason });
      if (!req) return;
      button.disabled = true;
      try {
        await api(req.path, { method: req.method, body: JSON.stringify(req.body) });
        await loadInbox();
        if (state.tab === "dashboard") renderDashboard();
      } catch (error) {
        button.disabled = false;
        window.alert(error.message || String(error));
      }
    };
  }
}

export function renderInbox() {
  const host = $("inbox");
  if (!host) return;
  if (inboxReport === undefined) { host.innerHTML = "loading…"; return; }
  const items = inboxRows((Date.now() - inboxReceivedAt) / 1000);
  if (!items.length) {
    host.innerHTML = inboxReport && renewalReport ? `<div class="empty">Nothing waiting on a person right now.</div>` : `<div class="err">Some attention sources are unavailable; the list may be incomplete.</div>`;
    return;
  }
  host.innerHTML = items.map(inboxItemRow).join("");
  if (!inboxReport) host.innerHTML += `<p class="sub">Process attention is unavailable; this list may omit process warnings.</p>`;
  if (!renewalReport) host.innerHTML += `<p class="sub">Renewal metadata is unavailable; this list may omit expiry warnings.</p>`;
  wireInboxRows(host);
}

/// The dashboard's own `inbox` view tile: a count and the top few items off
/// the same `inboxRows` the Inbox nav view reads -- reusing that fetch and
/// model rather than a second, scoped copy of either, so the tile and the
/// standalone Inbox can never disagree about what needs a person. Narrowed
/// to the rail's own selection with `inScope`, client-side,
/// the same way `renderDashboard` already narrows `tasks`/`scopes` and the
/// occupancy tiles narrow their rows -- every other tile on a scoped
/// dashboard reads the selection, and an item with no `scope` at all (a
/// lost standing agent, say) is never hidden by one. Clicks are wired by
/// `renderDashboard`, once, after `#dash`'s whole innerHTML (this tile
/// included) is set -- the same `wireInboxRows` helper `renderInbox` uses
/// for `#inbox`.
function inboxTile() {
  if (inboxReport === undefined) return dcard("Inbox", "", `<div class="inbox-list"><div class="empty">loading…</div></div>`);
  const items = inboxRows((Date.now() - inboxReceivedAt) / 1000).filter((it) => it.kind === "renewal" && it.scopes.length ? it.scopes.some(inScope) : !it.scope || inScope(it.scope));
  if (!items.length) return dcard("Inbox", "", `<div class="inbox-list"><div class="empty">${inboxReport && renewalReport ? "Nothing waiting on a person right now." : "Some attention sources are unavailable; the list may be incomplete."}</div></div>`);
  const top = items.slice(0, 5).map(inboxItemRow).join("");
  return dcard("Inbox", `${items.length} waiting`, `<div class="inbox-list">${top}</div>`);
}

// -------------------------------------------------------------------- wiring

/// The production-year's Runs/Scrap-share toggle is redrawn with the rest of
/// `#dash` on every render, so it is rewired every time -- unlike the window
/// selector below, which lives in the static bar and is wired once.
/// `renderDashboard` itself carries `.calwrap`'s scroll position across the
/// rebuild this causes, the same as it does for any other re-render.
function wireCalToggle() {
  const cal = $("cal-seg");
  if (!cal) return;
  for (const b of cal.querySelectorAll("button")) {
    b.onclick = () => {
      if (b.dataset.c === calMode) return;
      calMode = b.dataset.c;
      renderDashboard();
    };
  }
}

export function wireDashboard() {
  const btn = $("newTaskFromDash");
  if (btn) btn.onclick = () => openCreate();

  const customise = $("dashCustomise");
  if (customise) customise.onclick = () => openDashboardEditor();

  const seg = $("dash-window");
  if (seg) {
    for (const b of seg.querySelectorAll("button")) {
      b.onclick = () => {
        if (b.dataset.w === windowKey) return;
        windowKey = b.dataset.w;
        for (const o of seg.querySelectorAll("button")) o.classList.toggle("on", o.dataset.w === windowKey);
        reloadTileData();
      };
    }
  }
}

// ----------------------------------------------------- Customise (#160)

/// The scope this session is editing right now, or `null` between opens --
/// so `app.js`'s `DashboardChanged` handler can tell a refetch not to
/// clobber a draft in progress (`isDashboardEditorOpen`).
let editorScope = null;
let editorTiles = [];
let editorQuery = "";
/// `buildCatalogue`'s answer, off `/api/metrics`' `registry` -- fetched once
/// per open, never per keystroke; empty until that fetch lands, which is
/// also the shape a failed fetch leaves it in, so the catalogue side just
/// keeps showing "loading…" rather than a confusing empty search.
let editorCatalogue = [];
let editorSaving = false;

export function isDashboardEditorOpen() {
  return editorScope !== null;
}

/// Which scope Customise edits: the one selected in the rail, or the
/// instance root's own when nothing is selected -- the same fallback
/// `Request::Dashboard { scope: None }` reads, resolved by path
/// (`rootScopeName`, `dashboard-editor-model.js`) rather than by name.
/// `null` when the instance never opted itself into being a scope at all:
/// there is then no name the write side (`Request::DashboardSet`/
/// `DashboardReset`, unlike the read) could be given, so Customise has
/// nothing to edit.
function editingScopeName() {
  return state.scope || rootScopeName(state.scopes, state.root);
}

/// A tile's display name in the "Layout" column: `VIEW_LABELS` for a view
/// tile, or the catalogue's own title for a metric tile once it has loaded
/// (falling back to the bare id before then, or for a metric the registry
/// never named).
function editorTileLabel(tile) {
  if (tile.view) return VIEW_LABELS[tile.view] || tile.view;
  const found = editorCatalogue.find((e) => e.kind === "metric" && e.id === tile.metric);
  return (found && found.title) || tile.metric;
}

function editorHtml(offerReset) {
  return `
    <header><div><h2>Customise dashboard</h2><span class="sub">${esc(editorScope)}</span></div>
      <button class="x" id="de-close">&times;</button></header>
    <div class="body">
      <div class="de-cols">
        <div class="de-col">
          <h3>Layout</h3>
          <div id="de-tiles"></div>
        </div>
        <div class="de-col">
          <h3>Add a tile</h3>
          <input id="de-search" type="text" placeholder="Search views and metrics…">
          <div id="de-results"></div>
        </div>
      </div>
      <div class="err" id="de-err"></div>
      <div class="row-btns" style="margin-top:16px">
        <button class="btn primary" id="de-save">Save</button>
        ${offerReset ? '<button class="btn" id="de-reset">Reset to inherited</button>' : ""}
        <button class="btn" id="de-cancel">Cancel</button>
      </div>
    </div>`;
}

function closeEditor() {
  editorScope = null;
  closeModal();
}

function renderEditorTiles() {
  const host = $("de-tiles");
  if (!host) return;
  if (!editorTiles.length) {
    host.innerHTML = `<div class="empty">No tiles yet -- add one from the catalogue.</div>`;
    return;
  }
  const sizes = Object.keys(SIZES);
  host.innerHTML = editorTiles
    .map(
      (tile, i) => `
    <div class="de-tile">
      <span class="de-tile-name">${esc(editorTileLabel(tile))}</span>
      <select class="de-tile-size" data-i="${i}" title="size">
        ${sizes.map((s) => `<option value="${s}"${tile.size === s ? " selected" : ""}>${s}</option>`).join("")}
      </select>
      <button class="btn small" data-act="up" data-i="${i}"${i === 0 ? " disabled" : ""} title="move up">&uarr;</button>
      <button class="btn small" data-act="down" data-i="${i}"${i === editorTiles.length - 1 ? " disabled" : ""} title="move down">&darr;</button>
      <button class="btn small danger" data-act="remove" data-i="${i}">Remove</button>
    </div>`
    )
    .join("");
  for (const sel of host.querySelectorAll(".de-tile-size")) {
    sel.onchange = () => {
      editorTiles = setTileSize(editorTiles, Number(sel.dataset.i), sel.value);
      renderEditorBody();
    };
  }
  for (const b of host.querySelectorAll("button[data-act]")) {
    b.onclick = () => {
      const i = Number(b.dataset.i);
      if (b.dataset.act === "up") editorTiles = moveTileUp(editorTiles, i);
      else if (b.dataset.act === "down") editorTiles = moveTileDown(editorTiles, i);
      else editorTiles = removeTile(editorTiles, i);
      renderEditorBody();
    };
  }
}

function renderEditorResults() {
  const host = $("de-results");
  if (!host) return;
  if (!editorCatalogue.length) {
    host.innerHTML = `<div class="empty">loading…</div>`;
    return;
  }
  const results = searchCatalogue(editorCatalogue, editorQuery).slice(0, 40);
  if (!results.length) {
    host.innerHTML = `<div class="empty">No matches.</div>`;
    return;
  }
  host.innerHTML = results
    .map(
      (e) => `
    <div class="de-result">
      <span class="de-result-name">${esc(e.title)}<span class="de-result-sub"> · ${esc(e.subtitle)}</span></span>
      <button class="btn small" data-kind="${e.kind}" data-id="${esc(e.id)}">Add</button>
    </div>`
    )
    .join("");
  for (const b of host.querySelectorAll("button[data-id]")) {
    b.onclick = () => {
      const entry = editorCatalogue.find((e) => e.kind === b.dataset.kind && e.id === b.dataset.id);
      if (entry) editorTiles = addTile(editorTiles, entry, "m");
      renderEditorBody();
    };
  }
}

function renderEditorBody() {
  renderEditorTiles();
  renderEditorResults();
  const errors = editorErrors(editorTiles);
  const err = $("de-err");
  if (err && !editorSaving) err.textContent = errors[0] || "";
  const save = $("de-save");
  if (save) save.disabled = errors.length > 0 || editorSaving;
  const reset = $("de-reset");
  if (reset) reset.disabled = editorSaving;
}

/// Saves or resets, then refreshes the page's own layout state
/// (`layoutTiles`/`layoutSource`) from the daemon's answer directly --
/// `loadLayout` would refetch the same thing over the wire a second time,
/// and `Request::DashboardSet`/`DashboardReset` already answer with exactly
/// what a follow-up `Request::Dashboard` would.
async function submitEditor(method) {
  editorSaving = true;
  renderEditorBody();
  try {
    const answer = await api(`/api/dashboard?scope=${encodeURIComponent(editorScope)}`, {
      method,
      ...(method === "PUT" ? { body: JSON.stringify({ tiles: editorTiles }) } : {}),
    });
    layoutTiles = answer.tiles;
    layoutSource = answer.source;
    closeEditor();
    await loadTileData(resolveDashboard(layoutTiles));
    renderDashboard();
  } catch (e) {
    editorSaving = false;
    const err = $("de-err");
    if (err) err.textContent = e.message;
    renderEditorBody();
  }
}

function wireEditorChrome() {
  $("de-close").onclick = closeEditor;
  $("de-cancel").onclick = closeEditor;
  $("de-search").oninput = () => {
    editorQuery = $("de-search").value;
    renderEditorResults();
  };
  $("de-save").onclick = () => submitEditor("PUT");
  const reset = $("de-reset");
  if (reset) reset.onclick = () => submitEditor("DELETE");
}

/// Opens the editor over a working copy of what is on screen
/// (`seedTiles`), then fetches the metric registry once for the catalogue's
/// other half -- the view half (`VIEW_IDS`) needs no fetch at all. A scope
/// that resolves to nothing (the instance never opted into being a scope,
/// and none is selected) has no write target, so this is a no-op rather
/// than a modal with a Save button that can only ever fail.
async function openDashboardEditor() {
  const scope = editingScopeName();
  if (!scope) return;
  dropModal();
  editorScope = scope;
  editorTiles = seedTiles(layoutTiles);
  editorQuery = "";
  editorCatalogue = [];
  editorSaving = false;
  scrim(editorHtml(canReset(layoutSource, editorScope)));
  wireEditorChrome();
  renderEditorBody();

  let registry = [];
  try {
    registry = (await api("/api/metrics")).registry;
  } catch {
    registry = [];
  }
  if (!isDashboardEditorOpen()) return; // closed while the fetch was in flight
  editorCatalogue = buildCatalogue(registry);
  renderEditorBody();
}
