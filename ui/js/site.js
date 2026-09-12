//! The site plan: one data layer built from what the daemon actually serves,
//! and the isometric 2D renderer over it. `site-render.js` is the other
//! renderer over the same facts -- it imports `getScene()` from here rather
//! than owning any of this, because a hall, a worker or a queued run must mean
//! the same thing in both views.
//!
//! What a hall is made of:
//!   - its shape comes from `/api/site`: how many files, bytes and directories
//!     the scope holds, mapped to a tier, a floor count, a footprint and a
//!     number of window bays by `factory-core/src/building.rs`. Nothing here
//!     decides it -- the daemon serves the shape already worked out, the way
//!     it serves the occupancy chart already assembled, and the mapping is
//!     tested there rather than guessed at here;
//!   - what is lit on it comes from the same call: runs in flight, tasks
//!     queued and agents standing up become lit floors, a roof beacon and a
//!     beat. Size decides the structure and activity decides the light, and
//!     neither reaches the other -- a cue that could add a floor would cost
//!     height its meaning, and one that could widen a footprint would shove
//!     the hall's neighbours across the apron every time a run started;
//!   - its size on disk comes from `/api/site`, for the label and the floor;
//!   - its floor comes from the same call -- `areas`, the scope's top-level
//!     entries, squarified once here (`computeFloor`) and handed through
//!     `getScene()` so the plan and the lit render treemap nothing twice and
//!     place the same directory in the same corner. A scope that could not
//!     be walked keeps the hatch and "floor not recorded"; one that was
//!     walked and found empty gets a plain floor and says "floor empty" --
//!     `size_bytes` already draws that line between "unknown" and "really
//!     zero", and the floor has to keep it, not flatten the two into one
//!     drawing. A truncated walk draws its floor and says it is partial,
//!     because its proportions are a lower bound, not a measurement
//!     (`site.rs`'s `ENTRY_CAP`);
//!   - its state and its workers come from `/api/agents` -- an `AgentView`'s
//!     `active` runs are the only bays Factory has, because Factory has no
//!     bay: "a row is an agent, not a bay" (`occupancy.rs`). A figure is drawn
//!     per agent that is actually present, not per slot a scope could fill.
//!   - the runs queued at its door come from `state.tasks`, filtered to this
//!     scope's pending tasks -- the same list the Tasks view already holds.
//!
//! What this does not draw, and why: no file a session is editing (`FOCUS` in
//! the prototype -- not anything Factory records, full stop); no cross-scope
//! workflow trace (Factory has no hand-off graph to draw one from).

import { $, esc, api, state, since, statusBadge } from "./core.js";
import { inScope, scopeLabel } from "./scopes.js";
import { agentTags } from "./agents.js";
import { openTask } from "./tasks.js";
import { writeHash } from "./scopes.js";

// ------------------------------------------------------------- the data layer

const TW = 64, TH = 32, ZH = 26;
const GAP = 1.6;

let SITE = [];          // halls, in declared scope order
let byId = {};
let ZONE = null;
let AISLE = [];
let SPURS = [];
let footprintByName = {};    // scope name -> bytes, or null when unknown
let areasByName = {};        // scope name -> { areas: ScopeArea[], truncated }
let hallByName = {};         // scope name -> the whole ScopeFootprint the daemon served
let footprintLoaded = false;

/// A hall's height, in grid units: one storey each, plus the ground floor's
/// own headroom. Eight floors is the tallest `building.rs` will ever ask for,
/// which keeps the tallest hall inside the envelope the site was drawn for --
/// `pick()` hit-tests against `b.h`, and the camera frames the site from the
/// footprints.
const FLOOR_H = 0.42;
const FLOOR_BASE = 0.5;

/// What a hall is drawn as before `/api/site` has answered, and for a scope
/// that call did not mention. Deliberately the same shape the daemon serves
/// for a scope it could not read: a plain building of no particular size,
/// rather than a small one, which would be a measurement nobody made.
const UNMEASURED = {
  shape: { tier: "unknown", score: 0, floors: 2, width_tenths: 40, depth_tenths: 34, bays: 4 },
  cues: { level: "idle", load_pct: 0, lit_floors: 0, beacon: "off", pulse_ms: 0, glow_pct: 0 },
  activity: { active_runs: 0, blocked_runs: 0, queued_tasks: 0, live_agents: 0, failed_agents: 0, declared_agents: 0 },
  metrics: { known: false, files: 0, source_files: 0, bytes: 0, directories: 0, truncated: false },
};

/// How far a hall has got towards the height and the lighting it was last
/// served, kept by scope name so a rebuild does not restart the animation.
/// Held here rather than on the hall objects because `buildSite` replaces
/// those wholesale every time an event lands.
const eased = {};

/// What the roof beacon is coloured by. The same four state colours the rest
/// of the page uses, in the same order of urgency `hallState` has always
/// applied: a person waited on comes first.
const BEACON_COLOUR = { blocked: "wait", fault: "fault", working: "run", waiting: "idle", off: "idle" };

/// What a lit window is coloured by, which is a different question -- and the
/// answer is: nothing. How far up a hall is lit says how *much* of it is
/// working; which kind of work that is belongs to the beacon, which is the one
/// thing on a hall that changes colour. A window is warm whether the work is
/// running or waiting at the door, because a light is on or it is not, and
/// lighting queued work in `--idle` would be a light nobody could see was on.
function litKey() { return "lit"; }
let sel = null;               // selected hall id

/// The floor's inset from the hall's own walls -- the same margin `shellBase`
/// already clips its hatch to, so the treemap and the fallback hatch occupy
/// exactly the same rectangle and opening a roof never shifts the floor.
const FLOOR_INSET = 0.15;

/// Colours the floor treemap cycles through -- a dedicated ramp (`--ly1`
/// through `--ly6`), read out of CSS the same way `readPalette` reads
/// everything else. Not the hall's own greys (`wallA`/`roof2`/`paint`/...):
/// those sit close enough to each other and to the ground around them that a
/// treemap painted from them reads as one slab. Not the hall-state colours
/// (`run`/`idle`/`wait`/`fault`) either -- those mean something specific
/// about a run, and a directory tile borrowing one would read as a claim
/// about work rather than about size.
const AREA_KEYS = ["ly1", "ly2", "ly3", "ly4", "ly5", "ly6"];

export const layers = { agents: true, flow: true, zones: true, labels: true };

/// `/api/site`, which now answers both halves of what a hall shows: the walk
/// of the scope's directory (cached by the daemon, because this is asked for
/// on every run and agent event now) and what Factory is doing in it, which is
/// the reason to ask again. A failure leaves the last answer standing rather
/// than emptying the site: a hall drawn from a minute-old walk is better than
/// one that shrinks to nothing because a fetch lost a race with a reload.
export async function loadFootprint() {
  try {
    const data = (await api("/api/site")).footprint;
    footprintByName = {};
    areasByName = {};
    hallByName = {};
    for (const s of data.scopes) {
      footprintByName[s.name] = s.size_bytes ?? null;
      areasByName[s.name] = { areas: s.areas || [], truncated: !!s.truncated };
      hallByName[s.name] = s;
    }
    footprintLoaded = true;
  } catch {
    // Keep whatever was already there; only the very first failure leaves the
    // site unmeasured, and `UNMEASURED` is what that draws.
  }
}

/// Squarified treemap (Bruls, Huizing & van Wijk): lays `values` -- each
/// carrying `.a`, an area already scaled so the values sum to `w * h` -- into
/// the rectangle `(x, y, w, h)`. At every step it grows the row it is
/// building by one more item only while that keeps the row's worst aspect
/// ratio the same or better, so tiles stay squarish instead of degrading
/// into slivers as the sizes get more lopsided; `items` must already be
/// sorted largest first, which is how `/api/site` already sends `areas`.
function squarifyRect(values, x, y, w, h) {
  const tiles = [];
  let items = values.slice();
  let rx = x, ry = y, rw = w, rh = h;

  function worst(row, side) {
    const sum = row.reduce((s, v) => s + v.a, 0);
    if (sum <= 0 || side <= 0) return Infinity;
    const maxA = Math.max(...row.map((v) => v.a));
    const minA = Math.max(1e-9, Math.min(...row.map((v) => v.a)));
    const s2 = side * side;
    return Math.max((s2 * maxA) / (sum * sum), (sum * sum) / (s2 * minA));
  }

  while (items.length) {
    const side = Math.min(rw, rh);
    let row = [items[0]];
    let k = 1;
    while (k < items.length) {
      const trial = row.concat(items[k]);
      if (worst(trial, side) <= worst(row, side)) { row = trial; k++; } else break;
    }
    const rowSum = row.reduce((s, v) => s + v.a, 0);
    const vertical = rw <= rh; // the row fills the short side, laid across it
    const thickness = vertical ? rowSum / Math.max(rw, 1e-9) : rowSum / Math.max(rh, 1e-9);
    let cursor = vertical ? rx : ry;
    row.forEach((v) => {
      const extent = thickness > 0 ? v.a / thickness : 0;
      if (vertical) {
        tiles.push({ name: v.name, size_bytes: v.size_bytes, x: cursor, y: ry, w: extent, h: thickness });
      } else {
        tiles.push({ name: v.name, size_bytes: v.size_bytes, x: rx, y: cursor, w: thickness, h: extent });
      }
      cursor += extent;
    });
    if (vertical) { ry += thickness; rh -= thickness; } else { rx += thickness; rw -= thickness; }
    items = items.slice(row.length);
  }
  return tiles;
}

/// The floor a hall's `areas` treemap onto, computed once so the plan and the
/// lit render both read it off `getScene()` rather than laying it out twice.
/// `[]` for a scope with no areas -- whether that is because it could not be
/// walked or because the walk found nothing is `size_bytes`'s distinction to
/// draw, not this function's; the caller reads that separately.
function computeFloor(areas, w, d) {
  const fw = w - FLOOR_INSET * 2, fd = d - FLOOR_INSET * 2;
  if (!areas || !areas.length || fw <= 0 || fd <= 0) return { tiles: [] };
  const total = areas.reduce((s, a) => s + a.size_bytes, 0);
  if (total <= 0) return { tiles: [] };
  const rectArea = fw * fd;
  const values = areas.map((a) => ({ name: a.name, size_bytes: a.size_bytes, a: (a.size_bytes / total) * rectArea }));
  const tiles = squarifyRect(values, FLOOR_INSET, FLOOR_INSET, fw, fd);
  tiles.forEach((t, i) => { t.colorKey = AREA_KEYS[i % AREA_KEYS.length]; });
  return { tiles };
}

function hallState(scope) {
  const active = scope.agents.flatMap((a) => a.active);
  // A run waiting on a person is the most urgent fact a hall can show, ahead
  // of work in progress; a standing agent that failed to start is a real
  // problem but a quieter one than either.
  if (active.some((r) => r.status === "blocked")) return "wait";
  if (active.some((r) => r.status === "running" || r.status === "dispatching")) return "run";
  if (scope.agents.some((a) => a.error)) return "fault";
  return "idle";
}

function workersFor(scope) {
  const out = [];
  for (const a of scope.agents) {
    const standing = a.lifetime !== "task";
    const present = standing && (a.state === "ready" || a.state === "starting");
    if (a.active.length) {
      for (const r of a.active) {
        out.push({
          agent: a.name,
          state: r.status === "blocked" ? "wait" : (r.status === "running" || r.status === "dispatching" ? "run" : "idle"),
          run: r,
        });
      }
    } else if (present) {
      out.push({ agent: a.name, state: a.error ? "fault" : "idle", run: null });
    } else if (a.error && standing) {
      out.push({ agent: a.name, state: "fault", run: null });
    }
  }
  return out;
}

function queuedAt(name) {
  return [...state.tasks.values()]
    .filter((t) => t.scope === name && t.status === "pending")
    .sort((a, b) => a.created_at.localeCompare(b.created_at));
}

/// How much depth a hall needs beyond its own footprint: its workers stand
/// just outside its door, in rows of their own, and the next row of halls
/// must clear all of them or a busy hall's agents end up standing inside its
/// neighbour.
///
/// Sized from `agents.length` -- what the scope *declares* -- never from how
/// many of them happen to be working right now. A run starting or ending
/// must not move a single hall: the apron is reserved whether or not anyone
/// is standing in it today, the same way the door's queue row is reserved
/// whether or not a run happens to be waiting there this poll.
function apron(b) {
  const perRow = Math.max(1, Math.floor(b.w / 1.05));
  const workerRows = Math.max(1, Math.ceil(Math.max(1, b.agents.length) / perRow));
  return 0.6 + (workerRows + 1) * 0.75;
}

/// Declared order, never sorted by size or state: the same scopes must land
/// in the same place on every poll, or the site reshuffles under someone's
/// cursor. Layout is a pure function of (order, size) and is never persisted.
function layoutHalls(scopes) {
  const cols = Math.max(1, Math.round(Math.sqrt(scopes.length)));
  let x = 0, y = 0, rowDepth = 0, col = 0;
  for (const b of scopes) {
    if (col >= cols) {
      x = 0;
      y += rowDepth + GAP;
      rowDepth = 0;
      col = 0;
    }
    b.x = x;
    b.y = y;
    x += b.w + GAP;
    rowDepth = Math.max(rowDepth, b.d + apron(b));
    col++;
  }
}

export function buildSite() {
  // The rail decides which halls stand. A selection is a narrower site, not a
  // different one: the same scopes, the same footprints, fewer buildings.
  SITE = state.scopes.filter((scope) => inScope(scope.name)).map((scope) => {
    const bytes = footprintByName[scope.name];
    const known = footprintLoaded && bytes != null;
    const served = hallByName[scope.name] || UNMEASURED;
    const shape = served.shape, cues = served.cues;
    // Tenths of a grid unit on the wire, so two daemons drawing the same
    // repository cannot disagree in the eighth decimal place.
    const w = shape.width_tenths / 10;
    const d = shape.depth_tenths / 10;
    const hT = FLOOR_BASE + shape.floors * FLOOR_H;
    const areaInfo = areasByName[scope.name] || { areas: [], truncated: false };
    // Where this hall had got to last time, so a rebuild picks the animation
    // up rather than starting it again -- `buildSite` runs on every event.
    const was = eased[scope.name] || { h: hT, lit: cues.lit_floors, glow: cues.glow_pct / 100 };
    return {
      id: scope.name,
      name: scope.name,
      path: scope.path,
      runtime: scope.runtime,
      defaultAgent: scope.default_agent,
      w,
      d,
      // `h`, `litF` and `glow` are where the hall is now; the `T` of each is
      // where it is going. `tickHalls` closes the gap, so a tier change grows
      // a storey and a run lights a floor instead of either snapping.
      h: was.h,
      hT,
      litF: was.lit,
      litT: cues.lit_floors,
      glow: was.glow,
      glowT: cues.glow_pct / 100,
      shape,
      cues,
      metrics: served.metrics,
      counted: served.activity,
      sizeBytes: known ? bytes : null,
      footprintKnown: known,
      state: hallState(scope),
      agents: scope.agents,
      workers: workersFor(scope),
      queued: queuedAt(scope.name),
      // Computed once, here, so the plan and the lit render treemap nothing
      // twice and place the same directory in the same corner of the floor.
      areas: areaInfo.areas,
      areasTruncated: areaInfo.truncated,
      floor: computeFloor(areaInfo.areas, w, d),
      open: 0,
      openT: 0,
    };
  });
  layoutHalls(SITE);
  // Worker positions are geometry, computed once here rather than twice --
  // the plan canvas and the lit render must put the same agent in the same
  // place, or "the same facts, two views" stops being true.
  SITE.forEach((b) => {
    const perRow = Math.max(1, Math.floor(b.w / 1.05));
    b.workers.forEach((wk, i) => {
      wk.gx = b.x + 0.5 + (i % perRow) * 1.05;
      wk.gy = b.y + b.d + 0.5 + Math.floor(i / perRow) * 0.7;
    });
  });
  byId = {};
  SITE.forEach((b) => { byId[b.id] = b; });

  if (!SITE.length) {
    ZONE = null;
    AISLE = [];
    SPURS = [];
    return;
  }
  const pad = 2.2;
  const minX = Math.min(...SITE.map((b) => b.x)) - pad;
  const maxX = Math.max(...SITE.map((b) => b.x + b.w)) + pad;
  const minY = Math.min(...SITE.map((b) => b.y)) - pad;
  const maxY = Math.max(...SITE.map((b) => b.y + b.d)) + pad * 2;
  ZONE = { x: minX, y: minY, w: maxX - minX, d: (maxY - pad) - minY };
  // One aisle along the bottom of the site, and a short spur from each hall's
  // door down to it -- circulation, not a claim about where work flows.
  const aisleY = maxY - pad * 0.6;
  AISLE = [[minX, aisleY], [maxX, aisleY]];
  SPURS = SITE.map((b) => [[b.x + b.w / 2, b.y + b.d], [b.x + b.w / 2, aisleY]]);
  if (!byId[sel]) sel = SITE[0].id;
}

/// Close the gap between where each hall is drawn and where the last answer
/// from the daemon put it. Called once per frame by whichever renderer is on
/// screen -- both of them draw the same halls, so neither may ease them on its
/// own or the two views would disagree about a hall mid-change.
///
/// Exponential, against real elapsed time rather than frames: the plan runs at
/// whatever `requestAnimationFrame` gives it and the lit render at the same,
/// but a tab that was in the background wakes with a large `dt` and must not
/// take a hundred frames to catch up.
///
/// The footprint is deliberately not eased. It is what `layoutHalls` places
/// the site from, so a hall growing wider would have to push its neighbours
/// along with it; a re-walk is five minutes apart at the closest, and a
/// footprint that steps once in that time is not what "flicker" means.
const EASE_TAU = 0.28;

/// A reader who has asked for less movement gets the same facts without the
/// travel: every hall is simply where it is going. The cues themselves are not
/// motion -- a lit floor is lit either way -- so nothing is lost but the
/// journey.
export const STILL = !!(window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches);

export function tickHalls(dt) {
  const k = STILL ? 1 : 1 - Math.exp(-Math.max(0, Math.min(dt, 1)) / EASE_TAU);
  SITE.forEach((b) => {
    b.h += (b.hT - b.h) * k;
    b.litF += (b.litT - b.litF) * k;
    b.glow += (b.glowT - b.glow) * k;
    if (Math.abs(b.hT - b.h) < 0.002) b.h = b.hT;
    if (Math.abs(b.litT - b.litF) < 0.004) b.litF = b.litT;
    if (Math.abs(b.glowT - b.glow) < 0.004) b.glow = b.glowT;
    eased[b.id] = { h: b.h, lit: b.litF, glow: b.glow };
  });
}

/// How lit floor `i` is, 0..1, counting from the ground. A fraction rather
/// than a flag: it is what lets the lights come up one storey at a time
/// instead of the whole facade switching at once.
export function floorLight(b, i) {
  return Math.max(0, Math.min(1, b.litF - i));
}

export function getScene() {
  return { SITE, byId, ZONE, AISLE, SPURS, layers, getSel: () => sel, setRoof, select };
}

export function getPalette() { return C; }

function openable(b) {
  return !!b;
}

function setRoof(b, want) {
  if (!openable(b)) return;
  SITE.forEach((o) => { if (o !== b) o.openT = 0; });
  b.openT = want ? 1 : 0;
  syncRoofBtn();
}

function syncRoofBtn() {
  const btn = $("site-roof");
  if (!btn) return;
  const b = byId[sel];
  btn.disabled = !b;
  btn.classList.toggle("on", !!(b && b.openT));
  btn.textContent = b && b.openT ? "Close roof" : "Open roof";
}

function select(id) {
  if (!byId[id]) return;
  sel = id;
  renderRail();
  syncRoofBtn();
}

// -------------------------------------------------------------- the renderer

let cv, ctx, tip;
let W = 0, H = 0, DPR = 1;
let cam = { x: 0, y: 0, z: 0.6 };
let C = {};
let dragging = false, lx = 0, ly = 0, moved = 0, hov = null, tween = null;
let t0 = 0, raf = null, paused = false, last2d = 0;
let renderMod = null, renderFailed = false, renderMode = 0;

function readPalette() {
  const s = getComputedStyle(document.documentElement);
  ["ground", "ground2", "plate", "paint", "roof", "roof2", "wallA", "wallB", "ink", "muted", "faint",
   "line", "signal", "run", "idle", "wait", "fault", "lit", "panel", "sunk",
   "ly1", "ly2", "ly3", "ly4", "ly5", "ly6"].forEach((k) => {
    C[k] = (s.getPropertyValue("--" + k) || "").trim() || "#888888";
  });
}

function P(gx, gy, gz) {
  gz = gz || 0;
  const sx = (gx - gy) * TW / 2, sy = (gx + gy) * TH / 2 - gz * ZH;
  return [(sx - cam.x) * cam.z + W / 2, (sy - cam.y) * cam.z + H / 2];
}
function unP(px, py) {
  const sx = (px - W / 2) / cam.z + cam.x, sy = (py - H / 2) / cam.z + cam.y;
  return [(sx / (TW / 2) + sy / (TH / 2)) / 2, (sy / (TH / 2) - sx / (TW / 2)) / 2];
}
function poly(pts, fill, stroke, lw) {
  ctx.beginPath();
  pts.forEach((p, i) => { const q = P(p[0], p[1], p[2] || 0); i ? ctx.lineTo(q[0], q[1]) : ctx.moveTo(q[0], q[1]); });
  ctx.closePath();
  if (fill) { ctx.fillStyle = fill; ctx.fill(); }
  if (stroke) { ctx.strokeStyle = stroke; ctx.lineWidth = (lw || 1) * Math.max(cam.z, 0.5); ctx.stroke(); }
}
function line(pts, stroke, lw, dash) {
  ctx.save();
  if (dash) ctx.setLineDash(dash.map((d) => d * cam.z));
  ctx.beginPath();
  pts.forEach((p, i) => { const q = P(p[0], p[1], p[2] || 0); i ? ctx.lineTo(q[0], q[1]) : ctx.moveTo(q[0], q[1]); });
  ctx.strokeStyle = stroke; ctx.lineWidth = (lw || 1) * cam.z; ctx.lineCap = "round"; ctx.lineJoin = "round";
  ctx.stroke(); ctx.restore();
}
function text(t, gx, gy, gz, font, color, align, dy) {
  const p = P(gx, gy, gz);
  ctx.font = font; ctx.fillStyle = color; ctx.textAlign = align || "center"; ctx.textBaseline = "middle";
  ctx.fillText(t, p[0], p[1] + (dy || 0));
}

const stateCol = { run: "run", wait: "wait", idle: "idle", fault: "fault", q: "idle" };

function drawGround() {
  if (!ZONE) return;
  poly([[ZONE.x - 6, ZONE.y - 6], [ZONE.x + ZONE.w + 6, ZONE.y - 6],
        [ZONE.x + ZONE.w + 6, ZONE.y + ZONE.d + 6], [ZONE.x - 6, ZONE.y + ZONE.d + 6]], C.ground, null);
  if (cam.z > 0.7) {
    ctx.save(); ctx.globalAlpha = 0.3;
    for (let i = Math.floor(ZONE.x - 6); i <= ZONE.x + ZONE.w + 6; i += 1) line([[i, ZONE.y - 6], [i, ZONE.y + ZONE.d + 6]], C.ground2, 0.6);
    for (let j = Math.floor(ZONE.y - 6); j <= ZONE.y + ZONE.d + 6; j += 1) line([[ZONE.x - 6, j], [ZONE.x + ZONE.w + 6, j]], C.ground2, 0.6);
    ctx.restore();
  }
  if (layers.zones) {
    poly([[ZONE.x, ZONE.y], [ZONE.x + ZONE.w, ZONE.y], [ZONE.x + ZONE.w, ZONE.y + ZONE.d], [ZONE.x, ZONE.y + ZONE.d]], C.plate, null);
    ctx.save(); ctx.globalAlpha = 0.7;
    line([[ZONE.x, ZONE.y], [ZONE.x + ZONE.w, ZONE.y], [ZONE.x + ZONE.w, ZONE.y + ZONE.d], [ZONE.x, ZONE.y + ZONE.d], [ZONE.x, ZONE.y]],
      C.signal, 1.6, [1.6, 1.1]);
    ctx.restore();
    if (layers.labels && cam.z > 0.5) {
      ctx.save(); ctx.globalAlpha = 0.85;
      text("ZONE · scopes", ZONE.x + 0.2, ZONE.y + ZONE.d - 0.2, 0,
        '700 ' + Math.max(10, 9 * cam.z + 4).toFixed(0) + 'px "IBM Plex Sans Condensed", sans-serif', C.signal, "left", -4);
      ctx.restore();
    }
  }
}

function drawAisle() {
  if (!layers.flow || AISLE.length < 2) return;
  ctx.save(); ctx.globalAlpha = 0.5; line(AISLE, C.paint, 15); ctx.restore();
  line(AISLE, C.ground2, 12);
  SPURS.forEach((sp) => {
    ctx.save(); ctx.globalAlpha = 0.5; line(sp, C.paint, 9); ctx.restore();
    line(sp, C.ground2, 7);
  });
}

function queueMarks() {
  SITE.forEach((b) => {
    if (!b.queued.length) return;
    const y = b.y + b.d + 1.0;
    ctx.save(); ctx.globalAlpha = 0.6;
    line([[b.x + 0.2, y - 0.4], [b.x + 0.2 + b.queued.length * 0.9, y - 0.4]], C.paint, 1.6, [0.5, 0.4]);
    ctx.restore();
    b.queued.forEach((t, i) => {
      const qx = b.x + 0.55 + i * 0.9, qy = y;
      poly([[qx - 0.3, qy - 0.3, 0.3], [qx + 0.3, qy - 0.3, 0.3], [qx + 0.3, qy + 0.3, 0.3], [qx - 0.3, qy + 0.3, 0.3]], C.idle, null);
    });
    if (layers.labels && cam.z > 1.0) {
      text("waiting to run", b.x + 0.2 + b.queued.length * 0.45, y + 0.5, 0.02,
        '500 ' + Math.max(8, 6.5 * cam.z + 2.5).toFixed(0) + 'px "IBM Plex Mono", monospace', C.faint);
    }
  });
}

function plinth(b) {
  const m = 0.3;
  poly([[b.x - m, b.y - m, 0.2], [b.x + b.w + m, b.y - m, 0.2], [b.x + b.w + m, b.y + b.d + m, 0.2], [b.x - m, b.y + b.d + m, 0.2]],
    C.plate, C.line, 1);
}

/// An area tile's name, drawn only once the tile is large enough on screen to
/// hold it -- the same "too small, leave it to the tooltip" rule the rest of
/// this renderer already applies, just measured per tile instead of per hall.
function floorLabel(t, b, alpha) {
  const fs = Math.max(8, Math.min(11, 7 * cam.z + 2));
  ctx.font = '600 ' + fs + 'px "IBM Plex Mono", monospace';
  const tw = ctx.measureText(t.name).width;
  // A diamond t.w wide and t.h deep in grid space projects to roughly this
  // many pixels across -- cheap enough to not need the tile's real corners.
  const wpx = (t.w + t.h) * (TW / 2) * cam.z;
  if (tw + 10 > wpx) return;
  const p = P(b.x + t.x + t.w / 2, b.y + t.y + t.h / 2, 0.09);
  // The same panel-chip vocabulary `label()` already uses for a hall's own
  // name -- palette colours, not new ones, so the chip reads the same way
  // over any tile colour and in either theme.
  ctx.save(); ctx.globalAlpha = alpha * 0.93;
  ctx.fillStyle = C.panel;
  ctx.fillRect(p[0] - tw / 2 - 4, p[1] - fs / 2 - 2, tw + 8, fs + 4);
  ctx.globalAlpha = alpha;
  ctx.strokeStyle = C.line; ctx.lineWidth = 1;
  ctx.strokeRect(p[0] - tw / 2 - 4, p[1] - fs / 2 - 2, tw + 8, fs + 4);
  ctx.fillStyle = C.ink; ctx.textAlign = "center"; ctx.textBaseline = "middle";
  ctx.fillText(t.name, p[0], p[1] + 1);
  ctx.restore();
}

function shellBase(b) {
  const open = b.open || 0;
  if (open < 0.04) return;
  const x = b.x, y = b.y, w = b.w, d = b.d;
  ctx.save(); ctx.globalAlpha = Math.min(1, open * 1.6);
  poly([[x, y, 0.04], [x + w, y, 0.04], [x + w, y + d, 0.04], [x, y + d, 0.04]], C.ground2, null);
  ctx.restore();
  const clipPts = [[x + FLOOR_INSET, y + FLOOR_INSET, 0.05], [x + w - FLOOR_INSET, y + FLOOR_INSET, 0.05],
    [x + w - FLOOR_INSET, y + d - FLOOR_INSET, 0.05], [x + FLOOR_INSET, y + d - FLOOR_INSET, 0.05]];
  const tiles = b.floor.tiles;
  if (tiles.length) {
    // The floor is served: treemap the scope's top-level entries onto it,
    // largest first, in the same corners `site-render.js` draws them --
    // both read `b.floor`, computed once in `buildSite`, rather than laying
    // the tiles out twice.
    const alpha = Math.min(1, open * 1.6);
    ctx.save(); ctx.globalAlpha = alpha;
    ctx.save();
    ctx.beginPath();
    clipPts.forEach((p, i) => { const q = P(p[0], p[1], p[2]); i ? ctx.lineTo(q[0], q[1]) : ctx.moveTo(q[0], q[1]); });
    ctx.closePath(); ctx.clip();
    tiles.forEach((t) => {
      const tx = x + t.x, ty = y + t.y;
      poly([[tx, ty, 0.05], [tx + t.w, ty, 0.05], [tx + t.w, ty + t.h, 0.05], [tx, ty + t.h, 0.05]],
        C[t.colorKey], C.line, 0.6);
    });
    ctx.restore();
    poly(clipPts, null, C.faint, 1);
    ctx.restore();
    if (layers.labels && open > 0.6 && cam.z > 1.0) {
      ctx.save(); ctx.globalAlpha = alpha;
      tiles.forEach((t) => floorLabel(t, b, alpha));
      ctx.restore();
    }
    // A truncated walk's proportions are a lower bound, not a measurement --
    // the floor still draws, but says so instead of passing off 40,000
    // entries of a much larger tree as the whole of it.
    if (b.areasTruncated && open > 0.6 && cam.z > 0.8) {
      ctx.save(); ctx.globalAlpha = open;
      text("floor partial — cap reached", x + w / 2, y + d - FLOOR_INSET - 0.3, 0.07,
        '500 ' + Math.max(9, 7.5 * cam.z + 3).toFixed(0) + 'px "IBM Plex Mono", monospace', C.faint);
      ctx.restore();
    }
  } else if (b.footprintKnown) {
    // Walked, and there was nothing there: a real floor, not an unknown one.
    // `size_bytes` already draws this line between empty and unreadable
    // (`Some(0)` here, `None` in the branch below) -- the floor has to keep
    // it too, or the two facts collapse into the same drawing.
    ctx.save(); ctx.globalAlpha = Math.min(1, open * 1.6);
    poly(clipPts, C.plate, C.faint, 1);
    ctx.restore();
    if (open > 0.6 && cam.z > 0.8) {
      ctx.save(); ctx.globalAlpha = open;
      text("floor empty", x + w / 2, y + d / 2, 0.07,
        '500 ' + Math.max(9, 7.5 * cam.z + 3).toFixed(0) + 'px "IBM Plex Mono", monospace', C.faint);
      ctx.restore();
    }
  } else {
    // The floor itself is not drawn: Factory could not walk this scope's
    // directory tree, so a hatch and a label say "not recorded" rather than
    // an invented treemap or an empty-looking void.
    ctx.save(); ctx.globalAlpha = Math.min(1, open * 1.6) * 0.8;
    ctx.save();
    ctx.beginPath();
    clipPts.forEach((p, i) => { const q = P(p[0], p[1], p[2]); i ? ctx.lineTo(q[0], q[1]) : ctx.moveTo(q[0], q[1]); });
    ctx.closePath(); ctx.clip();
    ctx.globalAlpha = Math.min(1, open * 1.6) * 0.18;
    const step = 0.5;
    for (let k = -d; k < w + d; k += step) line([[x + k, y, 0.05], [x + k + d, y + d, 0.05]], C.faint, 1);
    ctx.restore();
    poly(clipPts, null, C.faint, 1);
    ctx.restore();
    if (open > 0.6 && cam.z > 0.8) {
      ctx.save(); ctx.globalAlpha = open;
      text("floor not recorded", x + w / 2, y + d / 2, 0.07,
        '500 ' + Math.max(9, 7.5 * cam.z + 3).toFixed(0) + 'px "IBM Plex Mono", monospace', C.faint);
      ctx.restore();
    }
  }
}

function shellTop(b, hi) {
  const open = b.open || 0, x = b.x, y = b.y, w = b.w, d = b.d, h = b.h;
  const wh = h * (1 - open) + 0.44 * open;
  poly([[x + w, y, wh], [x + w, y + d, wh], [x + w, y + d, 0], [x + w, y, 0]], C.wallA, C.line, 1);
  poly([[x, y + d, wh], [x + w, y + d, wh], [x + w, y + d, 0], [x, y + d, 0]], C.wallB, C.line, 1);
  if (cam.z > 1.1 && open < 0.35) {
    // One row of panes per storey, on the two faces the camera can see, lit
    // from the ground up: how far up the lights go is how much of this scope's
    // capacity is working, and how many rows there are is how much repository
    // there is. The two are readable at once precisely because one is the
    // building and the other is the light on it.
    const floors = Math.max(1, b.shape.floors), bays = Math.max(2, b.shape.bays);
    const on = C[litKey(b)];
    const fh = wh / floors;
    const paneW = (w - 0.6) / bays, paneD = (d - 0.6) / bays;
    for (let f = 0; f < floors; f++) {
      const light = floorLight(b, f);
      const z0 = f * fh + fh * 0.28, z1 = f * fh + fh * 0.70;
      for (let i = 0; i < bays; i++) {
        const a = y + 0.3 + i * paneD, a2 = a + paneD * 0.62;
        const c = x + 0.3 + i * paneW, c2 = c + paneW * 0.62;
        const right = [[x + w, a, z1], [x + w, a2, z1], [x + w, a2, z0], [x + w, a, z0]];
        const front = [[c, y + d, z1], [c2, y + d, z1], [c2, y + d, z0], [c, y + d, z0]];
        poly(right, C.line, null);
        poly(front, C.line, null);
        if (light > 0.02) {
          // Drawn over the dark pane rather than instead of it, so a floor
          // coming on fades up through it instead of switching.
          ctx.save(); ctx.globalAlpha = light * (0.45 + 0.55 * b.glow);
          poly(right, on, null);
          poly(front, on, null);
          ctx.restore();
        }
      }
    }
    poly([[x + w / 2 - 0.45, y + d, 0.95], [x + w / 2 + 0.45, y + d, 0.95], [x + w / 2 + 0.45, y + d, 0], [x + w / 2 - 0.45, y + d, 0]],
      C.wallA, C.paint, 1);
  }
  const rh = h + open * 1.35, of = -1.1 * open;
  ctx.save(); ctx.globalAlpha = 1 - open * 0.22;
  poly([[x + of, y + of, rh], [x + w + of, y + of, rh], [x + w + of, y + d + of, rh], [x + of, y + d + of, rh]],
    hi ? C.roof2 : C.roof, C.line, 1);
  ctx.restore();
  if (hi) {
    ctx.save(); ctx.globalAlpha = 0.9;
    poly([[x, y, rh], [x + w, y, rh], [x + w, y + d, rh], [x, y + d, rh]], null, C.signal, 2.2);
    ctx.restore();
  }
  // A fault or a blocked run is worth seeing from across the site, the same
  // way the prototype's beacon worked -- just tied to a real `AgentView.error`
  // or a real blocked run instead of one hand-picked hall.
  if (b.cues.beacon !== "off" && b.cues.beacon !== "waiting") {
    // A run that is going makes the beacon beat, and more of them make it beat
    // faster; a blocked run or a failed agent holds it steady, because neither
    // is progress. The period is the daemon's, so both views beat together.
    const period = STILL ? 0 : b.cues.pulse_ms;
    const pulse = period > 0 ? 0.35 + 0.65 * Math.abs(Math.sin(performance.now() / (period / 2))) : 0.9;
    const p = P(x + 0.4, y + 0.4, rh + 0.5);
    ctx.save(); ctx.globalAlpha = pulse;
    ctx.beginPath(); ctx.arc(p[0], p[1], 5 * cam.z, 0, 6.283);
    ctx.fillStyle = C[BEACON_COLOUR[b.cues.beacon] || "idle"]; ctx.fill();
    ctx.restore();
  }
}

function worker(wk, gx, gy, t) {
  const col = C[stateCol[wk.state] || "idle"];
  const bob = wk.state !== "idle" ? Math.sin(t * 2.4 + gx) * 0.05 : 0;
  const p0 = P(gx, gy, 0), p1 = P(gx, gy, 0.62 + bob);
  ctx.save(); ctx.globalAlpha = 0.18;
  ctx.beginPath(); ctx.ellipse(p0[0], p0[1], 7 * cam.z, 3.4 * cam.z, 0, 0, 6.283); ctx.fillStyle = "#000"; ctx.fill();
  ctx.restore();
  ctx.strokeStyle = col; ctx.lineCap = "round"; ctx.lineWidth = 4.6 * cam.z;
  ctx.beginPath(); ctx.moveTo(p0[0], p0[1]); ctx.lineTo(p1[0], p1[1]); ctx.stroke();
  ctx.beginPath(); ctx.arc(p1[0], p1[1] - 3.2 * cam.z, 3.1 * cam.z, 0, 6.283); ctx.fillStyle = col; ctx.fill();
  if (wk.state === "wait") {
    const pulse = 0.5 + 0.5 * Math.sin(t * 3);
    ctx.save(); ctx.globalAlpha = 0.55 + 0.45 * pulse;
    const bp = P(gx, gy, 1.5);
    ctx.beginPath(); ctx.arc(bp[0], bp[1], 7.2 * cam.z, 0, 6.283); ctx.fillStyle = C.wait; ctx.fill();
    ctx.fillStyle = C.panel; ctx.font = (9 * cam.z) + 'px "IBM Plex Sans", sans-serif';
    ctx.textAlign = "center"; ctx.textBaseline = "middle"; ctx.fillText("?", bp[0], bp[1] + 0.5 * cam.z);
    ctx.restore();
  }
}

function label(b) {
  if (!layers.labels || cam.z < 0.5) return;
  const p = P(b.x + b.w / 2, b.y + b.d / 2, b.h + 0.55);
  const sub = hallSub(b);
  const fs = Math.max(10, Math.min(13, 9 * cam.z + 4));
  ctx.font = '600 ' + fs + 'px "IBM Plex Sans Condensed", sans-serif';
  const w1 = ctx.measureText(b.name).width;
  ctx.font = '400 ' + (fs - 2) + 'px "IBM Plex Mono", monospace';
  const w2 = cam.z > 0.72 ? ctx.measureText(sub).width : 0;
  const bw = Math.max(w1, w2) + 14, bh = cam.z > 0.72 ? fs * 2 + 11 : fs + 9;
  ctx.save();
  ctx.fillStyle = C.panel; ctx.globalAlpha = 0.93;
  ctx.beginPath(); ctx.rect(p[0] - bw / 2, p[1] - bh, bw, bh); ctx.fill();
  ctx.globalAlpha = 1; ctx.strokeStyle = C.line; ctx.lineWidth = 1; ctx.stroke();
  ctx.fillStyle = C[stateCol[b.state] || "idle"]; ctx.fillRect(p[0] - bw / 2, p[1] - bh, 2.5, bh);
  ctx.fillStyle = C.ink; ctx.textAlign = "center"; ctx.textBaseline = "top";
  ctx.font = '600 ' + fs + 'px "IBM Plex Sans Condensed", sans-serif';
  ctx.fillText(b.name, p[0], p[1] - bh + 4);
  if (cam.z > 0.72) { ctx.fillStyle = C.muted; ctx.font = '400 ' + (fs - 2) + 'px "IBM Plex Mono", monospace'; ctx.fillText(sub, p[0], p[1] - bh + fs + 6); }
  ctx.restore();
}

/// Scaled to whichever unit reads as a real number: a scope under 1 kB or
/// under 1 MB rounding to "0" would look exactly like the empty-scope case
/// `footprintKnown` already exists to tell apart from -- the whole reason
/// that flag exists is so this function is never the thing doing the lying.
/// The line under a hall's name, in both views. The tier and the floor count
/// are the size signal in words, so a reader can check the building against
/// the number rather than having to trust the drawing.
export function hallSub(b) {
  const size = b.footprintKnown ? mb(b.sizeBytes) : "size not recorded";
  const floors = `${b.shape.floors} floor${b.shape.floors === 1 ? "" : "s"}`;
  return `${b.shape.tier} · ${floors} · ${size}`;
}

export function mb(bytes) {
  if (bytes < 1000) return `${bytes} B`;
  const k = bytes / 1e3;
  if (k < 1000) return `${Math.round(k)} kB`;
  const m = bytes / 1e6;
  return `${m.toFixed(m < 10 ? 1 : 0)} MB`;
}

function draw(now) {
  raf = requestAnimationFrame(draw);
  if (paused || !cv || !cv.clientWidth) return;
  if (Math.abs(cv.clientWidth - W) > 1 || Math.abs(cv.clientHeight - H) > 1) resize();
  const t = (now - t0) / 1000;
  tickHalls(last2d ? (now - last2d) / 1000 : 0);
  last2d = now;
  SITE.forEach((b) => {
    b.open += ((b.openT || 0) - b.open) * 0.13;
    if (Math.abs((b.openT || 0) - b.open) < 0.002) b.open = b.openT || 0;
  });
  if (tween) {
    const k = Math.min(1, (now - tween.t0) / tween.dur), e = 1 - Math.pow(1 - k, 3);
    cam.x = tween.x0 + (tween.x1 - tween.x0) * e;
    cam.y = tween.y0 + (tween.y1 - tween.y0) * e;
    cam.z = tween.z0 + (tween.z1 - tween.z0) * e;
    updateZoomLabel();
    if (k >= 1) tween = null;
  }
  ctx.setTransform(DPR, 0, 0, DPR, 0, 0);
  ctx.clearRect(0, 0, W, H);
  ctx.fillStyle = C.ground2; ctx.fillRect(0, 0, W, H);
  if (!SITE.length) return;
  drawGround();
  drawAisle();
  if (layers.flow) queueMarks();

  const items = [];
  SITE.forEach((b) => {
    items.push({ d: b.x + b.y - 0.03, f: () => { plinth(b); shellBase(b); } });
    items.push({ d: b.x + b.y + b.w + b.d, f: () => { shellTop(b, b.id === sel || b.id === hov); } });
  });
  if (layers.agents) SITE.forEach((b) => {
    b.workers.forEach((wk) => {
      items.push({ d: wk.gx + wk.gy + 0.4, f: () => worker(wk, wk.gx, wk.gy, t) });
    });
  });
  items.sort((a, b2) => a.d - b2.d);
  items.forEach((i) => i.f());
  SITE.forEach((b) => label(b));
}

function resize() {
  DPR = Math.min(window.devicePixelRatio || 1, 2);
  const r = cv.getBoundingClientRect();
  W = r.width; H = r.height; cv.width = W * DPR; cv.height = H * DPR;
}

function pick(mx, my) {
  for (let i = SITE.length - 1; i >= 0; i--) {
    const b = SITE[i], g = unP(mx, my + b.h * ZH * cam.z);
    if (g[0] >= b.x && g[0] <= b.x + b.w && g[1] >= b.y && g[1] <= b.y + b.d) return b;
  }
  return null;
}

function updateZoomLabel() {
  const el = $("site-zoom");
  if (el) el.textContent = "zoom " + cam.z.toFixed(2) + "×";
}

function zoomTo(nz, mx, my) {
  nz = Math.max(0.2, Math.min(2.4, nz));
  if (mx === undefined) { mx = W / 2; my = H / 2; }
  const ix = (mx - W / 2) / cam.z + cam.x, iy = (my - H / 2) / cam.z + cam.y;
  cam.z = nz; cam.x = ix - (mx - W / 2) / nz; cam.y = iy - (my - H / 2) / nz;
  updateZoomLabel();
}

function fitSite() {
  if (!ZONE) { cam = { x: 0, y: 0, z: 0.6 }; updateZoomLabel(); return; }
  cam.x = ((ZONE.x + ZONE.w / 2) - (ZONE.y + ZONE.d / 2)) * TW / 2;
  cam.y = ((ZONE.x + ZONE.w / 2) + (ZONE.y + ZONE.d / 2)) * TH / 2;
  const spanPx = Math.max(ZONE.w, ZONE.d) * Math.max(TW, TH * 2);
  cam.z = Math.max(0.2, Math.min(1.1, (Math.min(W || 800, H || 600) * 0.85) / Math.max(spanPx, 1)));
  updateZoomLabel();
}

function focusOn(b) {
  const cx = ((b.x + b.w / 2) - (b.y + b.d / 2)) * TW / 2, cy = ((b.x + b.w / 2) + (b.y + b.d / 2)) * TH / 2 - b.h * 0.42 * ZH;
  tween = { x0: cam.x, y0: cam.y, z0: cam.z, x1: cx, y1: cy, z1: Math.max(cam.z, 1.15), t0: performance.now(), dur: 520 };
}

function bindInput() {
  cv.addEventListener("pointerdown", (e) => { dragging = true; moved = 0; lx = e.offsetX; ly = e.offsetY; cv.classList.add("drag"); cv.setPointerCapture(e.pointerId); });
  cv.addEventListener("pointerup", (e) => {
    dragging = false; cv.classList.remove("drag");
    if (moved < 5) {
      const b = pick(e.offsetX, e.offsetY);
      if (b) { select(b.id); focusOn(b); }
    }
  });
  cv.addEventListener("pointerleave", () => { dragging = false; hov = null; if (tip) tip.style.opacity = 0; cv.classList.remove("drag"); });
  cv.addEventListener("pointermove", (e) => {
    if (dragging) {
      const dx = e.offsetX - lx, dy = e.offsetY - ly;
      moved += Math.abs(dx) + Math.abs(dy);
      tween = null; cam.x -= dx / cam.z; cam.y -= dy / cam.z; lx = e.offsetX; ly = e.offsetY;
      if (tip) tip.style.opacity = 0;
      return;
    }
    const b = pick(e.offsetX, e.offsetY);
    hov = b ? b.id : null;
    // The floor sits near z=0, close enough that the hall's own pick offset
    // (which corrects for its roof height) would miss every tile -- reproject
    // the raw pointer instead, but only once a hall's roof is open enough to
    // show a floor at all.
    let tile = null;
    if (b && b.open > 0.6 && b.floor.tiles.length) {
      const g2 = unP(e.offsetX, e.offsetY);
      const lx = g2[0] - b.x, ly = g2[1] - b.y;
      tile = b.floor.tiles.find((t) => lx >= t.x && lx <= t.x + t.w && ly >= t.y && ly <= t.y + t.h) || null;
    }
    if (b && tip) {
      const size = b.footprintKnown ? mb(b.sizeBytes) : "size not recorded";
      const lines = [`${b.runtime} · ${b.agents.length} agent${b.agents.length === 1 ? "" : "s"} · ${size} · click to ${b.openT ? "close" : "open"} the roof`];
      // The hall tooltip already exists; a hovered tile names the directory
      // and its size on top of it rather than opening a second tooltip.
      if (tile) lines.unshift(`${tile.name} · ${mb(tile.size_bytes)}${b.areasTruncated ? " · partial" : ""}`);
      tip.innerHTML = `<b>${esc(b.name)}</b>` + lines.map((l) => `<span class="k">${esc(l)}</span>`).join("");
      tip.style.left = Math.min(e.offsetX + 14, W - 244) + "px";
      tip.style.top = (e.offsetY + 14) + "px";
      tip.style.opacity = 1;
    } else if (tip) tip.style.opacity = 0;
  });
  cv.addEventListener("wheel", (e) => { e.preventDefault(); tween = null; zoomTo(cam.z * (e.deltaY < 0 ? 1.12 : 0.893), e.offsetX, e.offsetY); }, { passive: false });
  window.addEventListener("resize", () => { if (cv && cv.clientWidth) resize(); });
}

// ----------------------------------------------------------- the inspector

function renderRail() {
  const el = $("site-rail");
  if (!el) return;
  const b = byId[sel];
  if (!b) { el.innerHTML = `<div class="empty">Nothing selected.</div>`; return; }
  const scope = b.agents; // AgentView[]

  // A hall's own badge is one word for the whole scope, not a task's status --
  // it borrows the same colour classes so "blocked" reads the same way here
  // as it does on the Tasks page, without implying this scope itself is a
  // task that failed.
  const hallBadge = {
    wait: ["s-blocked", "blocked"], run: ["s-running", "running"],
    fault: ["s-failed", "agent error"], idle: ["s-pending", "idle"],
  }[b.state];
  // Same facts the floor itself draws: nothing measured, a measurement that
  // found nothing, a real measurement, or one the walk had to cut short.
  const floorNote = !b.areas.length
    ? (b.footprintKnown ? "floor empty" : "floor detail not recorded")
    : b.areasTruncated
      ? "floor detail partial — the walk hit its cap before it finished"
      : `${b.areas.length} area${b.areas.length === 1 ? "" : "s"} on the floor`;
  let h = `<div class="scope"><div class="head">
      <h3>${esc(b.name)}</h3><span class="sub">${esc(b.path)}</span>
    </div>
    <div class="agent"><div class="line">
      <span class="badge ${hallBadge[0]}">${esc(hallBadge[1])}</span>
      <span class="sub">${esc(b.runtime)} · default agent ${esc(b.defaultAgent)}</span>
    </div>
    <div class="sub">${b.footprintKnown ? mb(b.sizeBytes) : "size not recorded"} on disk · ${esc(floorNote)}</div>
    </div>
    ${hallDetail(b)}`;

  if (b.queued.length) {
    h += `<div class="agent"><div class="line"><span class="nm">At the door</span>
      <span class="sub">assigned, waiting for a run to start</span></div>
      <div class="work">${b.queued.map((t) => `
        <div class="job" data-task="${esc(t.id)}">${statusBadge(t.status)} <span class="jt">${esc(t.title)}</span></div>`).join("")}
      </div></div>`;
  }

  h += scope.map((a) => `
    <div class="agent">
      <div class="line">
        <span class="nm">${esc(a.name)}</span>
        ${a.adapter !== a.name ? `<span class="sub">${esc(a.adapter)}</span>` : ""}
        ${a.lifetime !== "task" ? statusBadge(a.state) : ""}
        ${agentTags(a)}
      </div>
      <div class="sub">${esc(a.description)}</div>
      ${a.error ? `<div class="err">${esc(a.error)}</div>` : ""}
      ${a.active.length
        ? `<div class="work">${a.active.map((w) => `
            <div class="job" data-task="${esc(w.task_id)}" data-run="${esc(w.run_id)}">
              ${statusBadge(w.status)} <span class="jt">${esc(w.task_title)}</span>
              <span class="sub">attempt ${w.attempt} · ${esc(w.trigger)} · ${since(w.started_at)}</span>
            </div>`).join("")}</div>`
        : `<div class="work idle">${a.lifetime !== "task" && (a.state === "ready" || a.state === "starting") ? "up and waiting" : "nothing running"}</div>`}
    </div>`).join("");

  h += "</div>";
  el.innerHTML = h;
  for (const job of el.querySelectorAll(".job[data-task]")) {
    job.onclick = () => openTask(job.dataset.task, job.dataset.run);
  }
}

/// Why this hall is the size it is and why it is lit the way it is, in the
/// numbers both answers were worked out from. The drawing is a summary; this
/// is the working, and it is the thing to read when the summary looks wrong.
function hallDetail(b) {
  const m = b.metrics, a = b.counted, c = b.cues, sh = b.shape;
  const count = (n, one, many) => `${n} ${n === 1 ? one : many || one + "s"}`;
  const sizeRows = m.known
    ? `<div class="sub">${count(m.files, "file")} · ${count(m.source_files, "source file")} ·
         ${count(m.directories, "module")} · ${esc(mb(m.bytes))}</div>
       ${m.truncated ? `<div class="sub">counted to the walk's cap — the numbers are a lower bound</div>` : ""}`
    : `<div class="sub">this scope's directory could not be read, so the hall is drawn plain rather than small</div>`;

  const lit = c.lit_floors === 0
    ? (a.live_agents ? "dark" : "dark — nobody is in")
    : `${count(c.lit_floors, "floor")} of ${sh.floors} lit`;
  const beacon = {
    off: "beacon off", waiting: "work at the door", working: "beacon beating — a run is going",
    fault: "beacon steady — an agent failed to start", blocked: "beacon steady — a run wants a person",
  }[c.beacon];

  return `<div class="agent">
    <div class="line"><span class="nm">The building</span><span class="sub">how much repository</span></div>
    <div class="sub">${esc(sh.tier)} · ${count(sh.floors, "floor")} · ${(sh.width_tenths / 10).toFixed(1)}×${(sh.depth_tenths / 10).toFixed(1)} footprint · size ${sh.score}/1000</div>
    ${sizeRows}
    <div class="line" style="margin-top:8px"><span class="nm">The lights</span><span class="sub">what is happening now</span></div>
    <div class="sub">${esc(c.level)} · ${c.load_pct}% of ${count(a.declared_agents || 1, "agent")} ·
      ${count(a.active_runs, "run")} running${a.blocked_runs ? `, ${a.blocked_runs} blocked` : ""} ·
      ${count(a.queued_tasks, "task")} queued</div>
    <div class="sub">${esc(lit)} · ${esc(beacon)}</div>
  </div>`;
}

// ---------------------------------------------------------------- lifecycle

let initialised = false;

export async function showSite() {
  $("site-empty").hidden = true;
  $("site-wrap").hidden = false;
  const standing = state.scopes.filter((s) => inScope(s.name));
  if (!standing.length) {
    $("site-wrap").hidden = true;
    $("site-empty").hidden = false;
    $("site-empty").textContent = state.scope
      ? `Nothing to place on the apron: ${scopeLabel()} has no scope under it.`
      : "This instance declares no scopes. The apron is empty because there is nothing to place on it yet -- add a scope to .factory/config.yaml and a hall appears here the next time this view loads.";
    return;
  }
  if (!footprintLoaded) await loadFootprint();
  buildSite();
  if (!initialised) {
    init();
    initialised = true;
  }
  renderLegend();
  renderRail();
  syncRoofBtn();
  // A link straight to Render lands here with the stage finally in place.
  // `fitSite` first anyway: `setMode` can fall back to the plan view on a
  // machine with no WebGL, and an unfitted camera is what it would fall back to.
  if (wantedMode !== null) {
    wantedMode = null;
    fitSite();
    setMode(1);
    return;
  }
  if (renderMode === 1 && renderMod) {
    renderMod.start(getScene());
  } else {
    fitSite();
    resume();
  }
}

export function hideSite() {
  pause();
  if (renderMod) renderMod.stop();
}

/// Called from app.js whenever a task/run/agent event lands and the site is
/// the one thing on screen that has to stay current without a full reload.
export function refreshSite() {
  if (!state.scopes.some((s) => inScope(s.name))) return;
  buildSite();
  renderRail();
  // `start` tears the whole lit scene down and builds it again -- every hall,
  // the ground, the labels. That is right when the site itself changed, and
  // wrong on the far more common case of a run starting, which would restage
  // the city on every event and undo every transition mid-flight. `update`
  // says whether it could carry the new facts onto the scene standing.
  if (renderMode === 1 && renderMod) {
    if (!renderMod.update(getScene())) renderMod.start(getScene());
  }
}

/// Two groups, because a hall carries two signals and a legend that ran them
/// together would be the same mistake the drawing is trying not to make: what
/// the building *is*, then what is *lit* on it.
function renderLegend() {
  const el = $("site-legend");
  if (!el) return;
  el.innerHTML = `
    <div class="grp">the building &mdash; how much repository</div>
    <div class="row"><span class="sw sz"></span>floors and footprint: files, bytes, modules</div>
    <div class="row"><span class="sw sz sm"></span>taller and wider is a larger codebase</div>
    <div class="grp">the lights &mdash; what is happening now</div>
    <div class="row"><span class="sw lt"></span>lit floors: how much of the scope's agents are spoken for</div>
    <div class="row"><span class="sw" style="background:var(--run)"></span>beacon beating: a run is going, faster the more of them</div>
    <div class="row"><span class="sw" style="background:var(--wait)"></span>steady amber: blocked, waiting on a person</div>
    <div class="row"><span class="sw" style="background:var(--fault)"></span>steady red: an agent failed to start</div>`;
}

function pause() { paused = true; }
function resume() { paused = false; t0 = t0 || performance.now(); last2d = 0; }

/// three.js is vendored (`ui/vendor/three.min.js`, served by the daemon) so a
/// machine with no internet still has this mode; the module that uses it is
/// loaded lazily because most page loads never touch Render at all.
async function ensureRenderLoaded() {
  if (renderFailed) return false;
  if (renderMod) return true;
  try {
    const mod = await import("./site-render.js");
    const ok = await mod.boot($("site-render-cv"), { getScene, getPalette, select });
    if (!ok) { renderFailed = true; return false; }
    renderMod = mod;
    return true;
  } catch (e) {
    if (window.console && console.warn) console.warn("[factory] render mode unavailable:", e);
    renderFailed = true;
    return false;
  }
}

/// A render-mode failure is set here, never appended into `#site-legend`:
/// that element is rewritten wholesale on every `showSite()`, so a note
/// stitched into it would vanish the moment someone left this view and came
/// back -- silent absence, which the issue calls worse than an explicit "no".
/// This element belongs to no other renderer, so setting it is idempotent
/// however many times Render is retried.
function setRenderErr(text) {
  const el = $("site-render-err");
  if (el) el.textContent = text || "";
}

/// Which mode the URL names. Plan is the default and writes nothing: a link
/// should not carry a segment saying "the ordinary one".
export function siteMode() {
  return renderMode === 1 ? ["render"] : [];
}

/// Applied from the URL. A route is read long before this view has ever been
/// built -- `setMode` needs the stage, the canvases and the palette that `init`
/// puts in place -- so a wish for Render made too early is parked and honoured
/// by `showSite` once there is something to switch. Plan needs no parking: it is
/// where the view starts.
let wantedMode = null;
export function setSiteMode(name) {
  const m = name === "render" ? 1 : 0;
  if (initialised) { setMode(m); return; }
  if (m !== 1) return;
  // `renderMode` moves now even though nothing can be switched yet, so that the
  // URL the router writes at the end of the route already says `render`. Left at
  // 0 until `showSite` got round to it, the write would say `plan` and the mode
  // arriving a moment later would push a second entry for the place the link
  // already named.
  renderMode = 1;
  wantedMode = 1;
}

function setMode(m) {
  renderMode = m;
  const seg = $("site-rmode");
  for (const b of seg.querySelectorAll("button")) b.classList.toggle("on", Number(b.dataset.rm) === m);
  $("site-stage").classList.toggle("rm3", m === 1);
  $("site-plan-cv").classList.toggle("off", m === 1);
  $("site-render-cv").classList.toggle("off", m !== 1);
  if (tip) tip.classList.remove("on");
  if (m === 0) {
    if (renderMod) renderMod.stop();
    resume();
    updateZoomLabel();
    return;
  }
  setRenderErr("");
  pause();
  ensureRenderLoaded().then((ok) => {
    if (!ok) {
      setMode(0);
      setRenderErr("Render mode needs WebGL and did not load — staying on the plan view.");
      // A link that asked for Render on a machine that cannot draw it should
      // stop saying so. Replace: the page corrected itself, nobody navigated.
      writeHash(true);
      return;
    }
    renderMod.start(getScene());
  });
}

function init() {
  cv = $("site-plan-cv");
  ctx = cv.getContext("2d");
  tip = $("site-tip");
  readPalette();
  document.addEventListener("factory:theme", () => {
    readPalette();
    if (renderMod) renderMod.repaint();
  });
  resize();
  bindInput();

  $("site-roof").onclick = () => { const b = byId[sel]; if (b) setRoof(b, !b.openT); };
  $("site-zin").onclick = () => (renderMode === 1 && renderMod ? renderMod.zoom(1.25) : zoomTo(cam.z * 1.25));
  $("site-zout").onclick = () => (renderMode === 1 && renderMod ? renderMod.zoom(0.8) : zoomTo(cam.z * 0.8));
  $("site-zfit").onclick = () => (renderMode === 1 && renderMod ? renderMod.fit() : fitSite());
  document.querySelectorAll("#view-site [data-layer]").forEach((btn) => {
    btn.onclick = () => { const k = btn.dataset.layer; layers[k] = !layers[k]; btn.classList.toggle("on", layers[k]); };
  });
  $("site-rmode").addEventListener("click", (e) => {
    const b = e.target.closest("button");
    if (!b || b.dataset.rm === undefined) return;
    setMode(Number(b.dataset.rm));
    writeHash();
  });
  $("site-tod").addEventListener("click", (e) => {
    const b = e.target.closest("button");
    if (!b || !b.dataset.tod) return;
    for (const o of $("site-tod").querySelectorAll("button")) o.classList.toggle("on", o === b);
    if (renderMod) renderMod.setTOD(b.dataset.tod);
  });

  t0 = performance.now();
  raf = requestAnimationFrame(draw);
}
