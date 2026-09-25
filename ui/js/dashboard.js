//! The dashboard: what is true now, read from `state.tasks` and
//! `state.scopes` the way it always was, plus what this factory has been
//! doing, read from `/api/production` -- the one aggregate over run history
//! the daemon serves (`production.rs`). One request per window or scope
//! change, not one per card: every history-backed card below reads the same
//! answer, so the throughput chart and the sparklines under the KPIs can
//! never disagree with each other.
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
//! What this page will not draw, because the domain does not record it:
//! cost, unit cost, or a euro figure of any kind (a `Run` carries no model,
//! no tokens, no money -- see `run.rs`); a "verified" share (there is no
//! verification step); workflows in flight (nothing links one task run to
//! another); bays (a row is an agent, not a slot -- `occupancy.rs`); and a
//! daemon-down calendar tile (the store has no event log, so a quiet day and
//! a stopped daemon are the same shape of nothing and must not be drawn as
//! different facts). The production-year grid labels a day before this
//! instance's first recorded run "no record", not "before this factory
//! existed" -- the earliest run is a lower bound on how long the instance has
//! existed, not its birthday.
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
//! decides what needs a person any more; `operations.rs` does, once.

import { $, esc, api, state } from "./core.js";
import { inScope, routeHref, scopeLabel } from "./scopes.js";
import { fmtAge, inboxItems } from "./operations-model.js";
import { openTask } from "./tasks.js";
import { openCreate } from "./task-form.js";

/// The three presets the window selector offers. `bin` travels with every
/// request rather than being guessed from `minutes` server-side, so a caller
/// always gets exactly the granularity it draws (`production.rs`).
const WINDOWS = {
  day: { label: "Today", minutes: 24 * 60, bin: "hour" },
  d14: { label: "14 days", minutes: 14 * 24 * 60, bin: "day" },
  d90: { label: "90 days", minutes: 90 * 24 * 60, bin: "week" },
};
let windowKey = "d14";

/// The last answer from `/api/production`. Module-local, the way
/// `activity.js` keeps its own log: nothing outside this file draws history.
/// `undefined` means "not fetched yet", `null` means "the fetch failed" --
/// kept apart from "fetched, and genuinely nothing has ever finished" so the
/// three read as different cards rather than the same blank.
let production;

export async function loadDashboard() {
  await loadProduction();
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

export function renderDashboard() {
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

  el.innerHTML = `
    <div class="kpis">${kpis(tasks, scopes, prod, everFinished).join("")}</div>
    <div class="drow">
      <section class="dcard">
        <h3>Throughput<span class="r">${esc(WINDOWS[windowKey].label.toLowerCase())} · finished per ${esc(WINDOWS[windowKey].bin)} · reworked share at the base</span></h3>
        ${throughput(prod, everFinished)}
      </section>
      ${onTheLine(tasks, prod, everFinished)}
    </div>
    ${productionYear(prod, everFinished)}
    <div class="drow">
      <section class="dcard wide">
        <h3>By scope<span class="r">${scopes.length} scope${scopes.length === 1 ? "" : "s"}</span></h3>
        ${byScopeTable(tasks, scopes)}
      </section>
    </div>`;

  wireCalToggle();

  // Restore scroll after the new `.calwrap` (if any -- a card with nothing
  // finished yet draws no grid at all) has real dimensions to measure. A
  // viewport wide enough to show every week has nothing to scroll past, so
  // the edge and 0 are the same place and this is a no-op there.
  const newCal = el.querySelector(".calwrap");
  if (newCal) {
    newCal.scrollLeft = calWasAtEdge ? newCal.scrollWidth - newCal.clientWidth : calScrollLeft;
  }
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
/// historical series of its own -- "in flight" and "queued" are what is true
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
  const pending = tasks.filter((t) => t.status === "pending");
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
    kpi("Queued", pending.length, "waiting at a door, no agent yet"),
    kpi("Finished", finished, `${winLabel}${scrapped ? ` · ${scrapped} scrapped` : ""}`, { spark: finishedSpark }),
    kpi("Scrap rate", finished ? round1(scrapped / finished * 100) : "—", finished ? `of finished, ${winLabel}` : "nothing finished yet", { unit: finished ? "%" : "", spark: scrapSpark }),
    kpi("Reworked", finished ? round1(reworked / finished * 100) : "—", finished ? `of finished, ${winLabel}` : "nothing finished yet", { unit: finished ? "%" : "", spark: reworkSpark }),
    kpi("Scopes · agents", `${scopes.length} · ${agentCount}`, `${standingUp} standing agent${standingUp === 1 ? "" : "s"} up`),
  ];
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
/// not a second tally -- they are the In flight and Queued KPIs, split out
/// and explained, so the two can never quietly drift apart.
function onTheLine(tasks, prod, everFinished) {
  const running = tasks.filter((t) => t.status === "running" || t.status === "dispatching");
  const blocked = tasks.filter((t) => t.status === "blocked");
  const pending = tasks.filter((t) => t.status === "pending");
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
    ${stnGroup("Waiting at a door · assigned, no agent yet")}
    ${stnRow("Queued", pending.length, pending.length || 1, "idle")}
    ${stnGroup(`Left the line today${today ? ` · against ${today.finished} finished` : ""}`)}
    ${todayRows}
    <p class="dnote">${holding} above hold an agent -- the same count the In flight KPI shows. ${pending.length} more
      ${pending.length === 1 ? "is" : "are"} queued at a door -- the Queued KPI.${todayNote}</p>
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
    const active = ts.filter((t) => t.status === "running" || t.status === "dispatching" || t.status === "verifying" || t.status === "blocked").length;
    const failed = ts.filter((t) => t.status === "failed").length;
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

// --------------------------------------------------------------------- inbox

/// The last `/api/operations` answer the Inbox drew from. `undefined` until
/// the first fetch, `null` when it failed -- kept apart so "loading", "not
/// available" and "nothing waiting" read as three different things.
let inboxReport;
let inboxAsked = 0;
let inboxReceivedAt = 0; // when it arrived, on this browser's clock -- ages grow from there

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
  let answer;
  try {
    answer = (await api("/api/operations")).report;
  } catch {
    answer = null;
  }
  if (mine !== inboxAsked) return;
  inboxReport = answer;
  inboxReceivedAt = Date.now();
  renderInbox();
}

export function renderInbox() {
  const host = $("inbox");
  if (!host) return;
  if (inboxReport === undefined) { host.innerHTML = "loading…"; return; }
  if (inboxReport === null) {
    host.innerHTML = `<div class="err">What needs a person is not available right now.</div>`;
    return;
  }
  const items = inboxItems(inboxReport, (Date.now() - inboxReceivedAt) / 1000);
  if (!items.length) {
    host.innerHTML = `<div class="empty">Nothing waiting on a person right now.</div>`;
    return;
  }
  host.innerHTML = items.map((it) => {
    const href = it.task_id ? "" : it.kind === "liveness_lost" ? routeHref(null, "roster") : "";
    // The reason is the agent's own words when it gave any -- a blocked
    // run's question, a failure's last error -- and the daemon's otherwise.
    return `<div class="inbox-item"${it.task_id ? ` data-task="${esc(it.task_id)}"` : ""}${it.run_id ? ` data-run="${esc(it.run_id)}"` : ""}${href ? ` data-href="${esc(href)}"` : ""}>
      <span class="it-dot" style="background:var(--${it.tone})"></span>
      <span class="it-t"><b>${esc(it.title || it.agent || "")}</b> — ${esc(it.label)}${it.suspicion ? ` <span class="ops-suspect">suspicion</span>` : ""}
        <span class="sub">${esc(it.reason)}${it.scope ? ` · ${esc(it.scope)}` : ""}</span>
      </span>
      <span class="it-age">${esc(fmtAge(it.age))}</span>
    </div>`;
  }).join("");

  for (const row of host.querySelectorAll(".inbox-item")) {
    row.onclick = () => {
      if (row.dataset.task) openTask(row.dataset.task, row.dataset.run);
      else if (row.dataset.href) location.hash = row.dataset.href;
    };
  }
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

  const seg = $("dash-window");
  if (seg) {
    for (const b of seg.querySelectorAll("button")) {
      b.onclick = () => {
        if (b.dataset.w === windowKey) return;
        windowKey = b.dataset.w;
        for (const o of seg.querySelectorAll("button")) o.classList.toggle("on", o.dataset.w === windowKey);
        loadDashboard();
      };
    }
  }
}
