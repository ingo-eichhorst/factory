//! The site plan: one data layer built from what the daemon actually serves,
//! and the isometric 2D renderer over it. `site-render.js` is the other
//! renderer over the same facts -- it imports `getScene()` from here rather
//! than owning any of this, because a hall, a worker or a queued run must mean
//! the same thing in both views.
//!
//! What a hall is made of:
//!   - its footprint comes from `/api/site`, the scope's size on disk;
//!   - its state and its workers come from `/api/agents` -- an `AgentView`'s
//!     `active` runs are the only bays Factory has, because Factory has no
//!     bay: "a row is an agent, not a bay" (`occupancy.rs`). A figure is drawn
//!     per agent that is actually present, not per slot a scope could fill.
//!   - the runs queued at its door come from `state.tasks`, filtered to this
//!     scope's pending tasks -- the same list the Tasks view already holds.
//!
//! What this does not draw, and why: no floor treemap (a scope's top-level
//! directory sizes are not served, so the floor says "not recorded" rather
//! than guessing); no file a session is editing (`FOCUS` in the prototype --
//! not anything Factory records, full stop); no cross-scope workflow trace
//! (Factory has no hand-off graph to draw one from).

import { $, esc, api, state, since, statusBadge } from "./core.js";
import { inScope, scopeLabel } from "./scopes.js";
import { agentTags } from "./agents.js";
import { openTask } from "./tasks.js";

// ------------------------------------------------------------- the data layer

const TW = 64, TH = 32, ZH = 26;
const GAP = 1.6;

let SITE = [];          // halls, in declared scope order
let byId = {};
let ZONE = null;
let AISLE = [];
let SPURS = [];
let footprintByName = {};    // scope name -> bytes, or null when unknown
let footprintLoaded = false;
let sel = null;               // selected hall id

export const layers = { agents: true, flow: true, zones: true, labels: true };

/// `side(k)`, carried over from the prototype: a hall you can see is a hall
/// you can size. `k` is the scope's codebase in megabytes; a scope whose size
/// could not be read gets the same default as a 1 MB scope, so it still draws
/// a plausible hall rather than a point -- `footprintKnown` is what a tooltip
/// reads to say the number is a guess.
function side(k) {
  return Math.min(11, 2.6 + Math.sqrt(k) * 0.85);
}

async function loadFootprint() {
  try {
    const data = (await api("/api/site")).footprint;
    footprintByName = {};
    for (const s of data.scopes) footprintByName[s.name] = s.size_bytes ?? null;
  } catch {
    footprintByName = {};
  }
  footprintLoaded = true;
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
function apron(b) {
  const perRow = Math.max(1, Math.floor(b.w / 1.05));
  const workerRows = b.workers.length ? Math.ceil(b.workers.length / perRow) : 0;
  const doorRows = b.queued.length ? 1 : 0;
  return 0.6 + (workerRows + doorRows) * 0.75;
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
    const k = known ? Math.max(bytes / 1e6, 0.02) : 1;
    const s = side(k);
    const agentCount = Math.max(1, scope.agents.length);
    return {
      id: scope.name,
      name: scope.name,
      path: scope.path,
      runtime: scope.runtime,
      defaultAgent: scope.default_agent,
      w: Math.round(s * 1.05 * 100) / 100,
      d: Math.round(s * 0.9 * 100) / 100,
      h: Math.min(3.3, 2.1 + 0.3 * Math.sqrt(agentCount)),
      sizeBytes: known ? bytes : null,
      footprintKnown: known,
      state: hallState(scope),
      agents: scope.agents,
      workers: workersFor(scope),
      queued: queuedAt(scope.name),
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
let t0 = 0, raf = null, paused = false;
let renderMod = null, renderFailed = false, renderMode = 0;

function readPalette() {
  const s = getComputedStyle(document.documentElement);
  ["ground", "ground2", "plate", "paint", "roof", "roof2", "wallA", "wallB", "ink", "muted", "faint",
   "line", "signal", "run", "idle", "wait", "fault", "panel", "sunk"].forEach((k) => {
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

function shellBase(b) {
  const open = b.open || 0;
  if (open < 0.04) return;
  const x = b.x, y = b.y, w = b.w, d = b.d;
  ctx.save(); ctx.globalAlpha = Math.min(1, open * 1.6);
  poly([[x, y, 0.04], [x + w, y, 0.04], [x + w, y + d, 0.04], [x, y + d, 0.04]], C.ground2, null);
  ctx.restore();
  // The floor itself is not drawn: Factory does not read a scope's directory
  // tree, so a hatch and a label say "not recorded" rather than an invented
  // treemap or an empty-looking void.
  ctx.save(); ctx.globalAlpha = Math.min(1, open * 1.6) * 0.8;
  ctx.save();
  const clipPts = [[x + 0.15, y + 0.15, 0.05], [x + w - 0.15, y + 0.15, 0.05], [x + w - 0.15, y + d - 0.15, 0.05], [x + 0.15, y + d - 0.15, 0.05]];
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

function shellTop(b, hi) {
  const open = b.open || 0, x = b.x, y = b.y, w = b.w, d = b.d, h = b.h;
  const wh = h * (1 - open) + 0.44 * open;
  poly([[x + w, y, wh], [x + w, y + d, wh], [x + w, y + d, 0], [x + w, y, 0]], C.wallA, C.line, 1);
  poly([[x, y + d, wh], [x + w, y + d, wh], [x + w, y + d, 0], [x, y + d, 0]], C.wallB, C.line, 1);
  if (cam.z > 1.1 && open < 0.35) {
    const lit = b.state === "run" || b.state === "wait";
    const n = Math.max(1, Math.round(d * 1.2));
    for (let i = 0; i < n; i++) {
      const a = y + 0.35 + i * (d - 0.5) / n, bb = a + (d - 0.5) / n * 0.55;
      poly([[x + w, a, wh * 0.72], [x + w, bb, wh * 0.72], [x + w, bb, wh * 0.42], [x + w, a, wh * 0.42]],
        lit ? C[stateCol[b.state]] : C.line, null);
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
  if (b.state === "wait" || b.state === "fault") {
    const pulse = 0.35 + 0.65 * Math.abs(Math.sin(performance.now() / 380));
    const p = P(x + 0.4, y + 0.4, rh + 0.5);
    ctx.save(); ctx.globalAlpha = pulse;
    ctx.beginPath(); ctx.arc(p[0], p[1], 5 * cam.z, 0, 6.283); ctx.fillStyle = C[stateCol[b.state]]; ctx.fill();
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
  const size = b.footprintKnown ? mb(b.sizeBytes) : "size not recorded";
  const sub = `${b.agents.length} agent${b.agents.length === 1 ? "" : "s"} · ${size}`;
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

function mb(bytes) {
  const m = bytes / 1e6;
  return m >= 1 ? `${m.toFixed(m < 10 ? 1 : 0)} MB` : `${Math.round(bytes / 1e3)} kB`;
}

function draw(now) {
  raf = requestAnimationFrame(draw);
  if (paused || !cv || !cv.clientWidth) return;
  if (Math.abs(cv.clientWidth - W) > 1 || Math.abs(cv.clientHeight - H) > 1) resize();
  const t = (now - t0) / 1000;
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
    if (b && tip) {
      const size = b.footprintKnown ? mb(b.sizeBytes) : "size not recorded";
      const sub = `${b.runtime} · ${b.agents.length} agent${b.agents.length === 1 ? "" : "s"} · ${size} · click to ${b.openT ? "close" : "open"} the roof`;
      tip.innerHTML = `<b>${esc(b.name)}</b><span class="k">${esc(sub)}</span>`;
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
  let h = `<div class="scope"><div class="head">
      <h3>${esc(b.name)}</h3><span class="sub">${esc(b.path)}</span>
    </div>
    <div class="agent"><div class="line">
      <span class="badge ${hallBadge[0]}">${esc(hallBadge[1])}</span>
      <span class="sub">${esc(b.runtime)} · default agent ${esc(b.defaultAgent)}</span>
    </div>
    <div class="sub">${b.footprintKnown ? mb(b.sizeBytes) : "size not recorded"} on disk · floor detail not recorded</div>
    </div>`;

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
  if (renderMode === 1 && renderMod) renderMod.start(getScene());
}

function renderLegend() {
  const el = $("site-legend");
  if (!el) return;
  el.innerHTML = `
    <div class="row"><span class="sw" style="background:var(--run)"></span>running</div>
    <div class="row"><span class="sw" style="background:var(--wait)"></span>blocked &mdash; waiting on a person</div>
    <div class="row"><span class="sw" style="background:var(--idle)"></span>idle, or up and waiting</div>
    <div class="row"><span class="sw" style="background:var(--fault)"></span>an agent failed to start</div>`;
}

function pause() { paused = true; }
function resume() { paused = false; t0 = t0 || performance.now(); }

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
  pause();
  ensureRenderLoaded().then((ok) => {
    if (!ok) {
      setMode(0);
      const note = $("site-legend");
      if (note) note.insertAdjacentHTML("afterbegin",
        `<div class="row" style="color:var(--fault)">Render mode needs WebGL and did not load — staying on the plan view.</div>`);
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
    if (b && b.dataset.rm !== undefined) setMode(Number(b.dataset.rm));
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
