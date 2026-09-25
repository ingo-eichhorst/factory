//! Pure shaping logic for the L4 Process Operations tab (`#106`) and the
//! Dashboard's Inbox -- turns an `OperationsReport` (see
//! `factory_core::operations` and `Payload::Operations` in `protocol.rs`)
//! into rows, chart geometry, request bodies and the "since you last looked"
//! difference `operations.js` and `dashboard.js` draw or send. Nothing here
//! touches `document` or the network, the same boundary `scenarios-model.js`
//! and `goals-model.js` keep, so a Node test needs no DOM at all.
//!
//! What this file never does is decide anything the report already
//! decided: which runs are exceptions, how severe they are, which actions
//! each allows, what a pace is and whose history judged it. Those are the
//! daemon's words (`operations.rs`), made once so the tab, `factory stats`
//! and the Inbox agree; this file only puts them on a page. The two things
//! it does work out for itself are layout -- scales, jitter, ticks -- and
//! the browser's own memory of what it showed last time, which is a
//! convenience and never a record.

// ------------------------------------------------------------ vocabulary
//
// Mirrors `ExceptionKind`, `Action`, `Stage`, `Pace`, `ScheduleState` and
// `FailKind` -- closed sets compiled into `factory-core`, sent as snake_case
// ids and never with a title, the same way `scenarios-model.js` mirrors
// `driver_defs()`. An id this table has never heard of falls back to itself
// with the underscores spaced, rather than vanishing.

export const KIND_LABELS = {
  blocked: "blocked",
  suspected_stuck: "may be stuck",
  failed_exhausted: "failed, no retry left",
  aging: "running long",
  schedule_late: "schedule late",
  schedule_missed: "slots missed",
  liveness_lost: "session gone",
  triggered_signpost: "signpost triggered",
};

export const ACTION_LABELS = {
  run_again: "Run again",
  cancel: "Cancel",
  answer: "Answer",
  run_now: "Run now",
  skip_next: "Skip next",
  pause_schedule: "Pause schedule",
  resume_schedule: "Resume schedule",
};

export const STAGES = ["queued", "dispatching", "running", "blocked"];

/// Green below p50 to red above p95 -- the order `Pace` itself sorts in.
export const PACES = ["below_p50", "p50_to_p70", "p70_to_p85", "p85_to_p95", "above_p95"];

export const PACE_LABELS = {
  below_p50: "below p50",
  p50_to_p70: "p50–p70",
  p70_to_p85: "p70–p85",
  p85_to_p95: "p85–p95",
  above_p95: "above p95",
};

export const FAIL_KIND_LABELS = {
  ack_timeout: "never acknowledged",
  run_timeout: "ran past its timeout",
  blocked_timeout: "blocked past its timeout",
  session_gone: "session gone",
  agent_failed: "agent reported failure",
  stop_failure: "harness stop failure",
  dispatch_failed: "dispatch refused",
  turn_ended: "turn ended unreported",
  cancelled_by_person: "cancelled by a person",
  cancelled_by_agent: "cancelled by an agent",
  cancelled_with_parent: "cancelled with its workflow or bench",
};

export function words(id) {
  return String(id ?? "").replace(/_/g, " ");
}

export function kindLabel(kind) {
  return KIND_LABELS[kind] || words(kind);
}

export function actionLabel(action) {
  return ACTION_LABELS[action] || words(action);
}

export function failKindLabel(kind) {
  return FAIL_KIND_LABELS[kind] || words(kind);
}

// ------------------------------------------------------------------ time

/// Seconds as a person reads a wait: `45s`, `12m`, `3.2h`, `4d`. The same
/// steps `core.js`'s `shortSpan` takes, carried on to days, since an aging
/// run or a missed slot can be days old and `72h` reads worse than `3d`.
export function fmtAge(secs) {
  if (secs === null || secs === undefined || !Number.isFinite(secs)) return "—";
  const s = Math.max(0, secs);
  if (s < 60) return `${Math.round(s)}s`;
  if (s < 5400) return `${Math.round(s / 60)}m`;
  if (s < 36 * 3600) return `${(s / 3600).toFixed(s < 36000 ? 1 : 0)}h`;
  return `${(s / 86400).toFixed(s < 10 * 86400 ? 1 : 0)}d`;
}

/// How old something is now: the age the daemon measured on its own clock,
/// plus how long ago its answer arrived on this one. Never the browser's
/// clock minus the server's timestamp -- two machines' clocks disagree, and
/// the difference would show up as age.
export function ageNow(ageS, elapsedS) {
  return Math.max(0, (ageS || 0) + Math.max(0, elapsedS || 0));
}

/// The server's time now, read the same way: when it answered, plus how
/// long ago that was here. What "since you last looked" is stamped with, so
/// it compares against run end times on the same clock.
export function serverNow(generatedAt, elapsedS) {
  return new Date(Date.parse(generatedAt) + Math.max(0, elapsedS || 0) * 1000).toISOString();
}

/// Timestamps compared as instants, never as strings: two RFC 3339 texts
/// with different fractional digits or offsets sort wrongly as text.
export function later(a, b) {
  return Date.parse(a) > Date.parse(b);
}

/// `2026-09-25 11:10 UTC` -- fixed, never the browser's locale, the same
/// rule `dashboard.js`'s `dmy` and `factory stats` keep, so a schedule reads
/// the same on the page as it does in the terminal.
export function utcStamp(iso) {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "—";
  const p = (n) => String(n).padStart(2, "0");
  return `${d.getUTCFullYear()}-${p(d.getUTCMonth() + 1)}-${p(d.getUTCDate())} ${p(d.getUTCHours())}:${p(d.getUTCMinutes())} UTC`;
}

/// `25.9.` -- a day on a chart axis.
export function dayLabel(iso) {
  const d = new Date(iso);
  return `${d.getUTCDate()}.${d.getUTCMonth() + 1}.`;
}

// ------------------------------------------------------------- attention

const SEVERITY_RANK = { high: 3, medium: 2, low: 1 };
const SEVERITY_TONE = { high: "fault", medium: "wait", low: "idle" };

/// One identity per exception across two reads of the report: the same
/// run (or task, or agent, or scenario) in the same kind of trouble since
/// the same moment is the same exception. Used for "since you last looked"
/// and for keeping a bulk selection stable across a live refetch.
export function exceptionKey(e) {
  const who = e.run_id || e.task_id || e.agent || e.title || "";
  return `${e.kind}|${who}|${e.since}`;
}

/// The attention queue as the page draws it: ages carried forward by
/// `elapsedS` (how long ago the answer arrived), and sorted the way the
/// daemon sorted it (severity, then oldest) so a re-render between two
/// fetches does not reshuffle rows. The daemon has already narrowed the
/// report to the selected scope and its subtree.
export function attentionRows(report, elapsedS = 0) {
  const list = (report && report.attention) || [];
  return list
    .map((e) => ({
      ...e,
      key: exceptionKey(e),
      label: kindLabel(e.kind),
      tone: SEVERITY_TONE[e.severity] || "idle",
      age: ageNow(e.age_s, elapsedS),
      alsoLabels: (e.also || []).map(kindLabel),
    }))
    .sort((a, b) => (SEVERITY_RANK[b.severity] || 0) - (SEVERITY_RANK[a.severity] || 0) || b.age - a.age || a.key.localeCompare(b.key));
}

/// The Inbox: the same exceptions, every scope, only what a person has to
/// act on. A triggered signpost is an observation -- something noticed,
/// not something wrong -- so it stays on the Operations tab and the
/// Scenarios tab, never in a person's to-do list.
export function inboxItems(report, elapsedS = 0) {
  return attentionRows(report, elapsedS).filter((e) => !e.observation);
}

// ------------------------------------------------- since you last looked
//
// Kept per browser, in `localStorage`, and a convenience only: a private
// window, a cleared site, or a second browser all start from "first visit".
// Nothing here is a record of who saw what.

export const SEEN_KEY = "factory-ops-seen";

/// One snapshot per rail selection: a report is the selected subtree's, so
/// a snapshot taken under one selection and compared under another would
/// call the other scopes' trouble new or resolved.
export function seenKey(scope) {
  return `${SEEN_KEY}:${scope === null || scope === undefined ? "*" : scope}`;
}

/// What to remember when the page stops being looked at. `at` is the
/// server's time (`serverNow`), so it compares with run end times.
export function seenSnapshot(rows, at, previous) {
  const newest = rows.reduce((m, r) => (!m || later(r.since, m) ? r.since : m), "");
  return {
    at,
    keys: rows.map((r) => r.key),
    // "Nothing needs you." says when the last exception was, and the report
    // cannot: an empty queue carries no dates. So the newest one this
    // browser ever saw is kept, labelled as exactly that.
    last_exception_at: newest || (previous && previous.last_exception_at) || null,
  };
}

/// Parse what `seenSnapshot` wrote, tolerating anything a hand or an older
/// page left there.
export function readSeen(raw) {
  if (!raw) return null;
  try {
    const v = JSON.parse(raw);
    if (!v || typeof v.at !== "string" || !Array.isArray(v.keys)) return null;
    return { at: v.at, keys: v.keys.filter((k) => typeof k === "string"), last_exception_at: typeof v.last_exception_at === "string" ? v.last_exception_at : null };
  } catch {
    return null;
  }
}

/// The shift handover, as counts: exceptions that are here now and were not
/// then, ones that were there then and are gone now, and runs that finished
/// after then. `finished` only covers the report's own health window; when
/// the last look was before it, `beyondWindow` says so rather than the
/// count pretending to reach back further.
export function sinceLastLooked(seen, rows, finishedRuns, windowFrom) {
  if (!seen) return { first: true };
  const before = new Set(seen.keys);
  const now = new Set(rows.map((r) => r.key));
  const fresh = rows.filter((r) => !before.has(r.key)).length;
  const resolved = [...before].filter((k) => !now.has(k)).length;
  const finished = (finishedRuns || []).filter((r) => later(r.ended_at, seen.at)).length;
  return {
    first: false,
    at: seen.at,
    fresh,
    resolved,
    finished,
    beyondWindow: !!(windowFrom && later(windowFrom, seen.at)),
  };
}

// ---------------------------------------------------------------- actions

/// What one action sends, to which route -- the slice-2 routes, each with
/// the reason in the body so the journal can say who asked and why. Pure,
/// so the mapping is tested without a daemon. `answer` carries its text in
/// the body and nowhere else: it is typed into the run's own session and
/// never journaled. `skip_next` names the slot it means (`row.next_run_at`).
export function actionRequest(action, row, { reason, text } = {}) {
  const why = (reason || "").trim();
  const withReason = why ? { reason: why } : {};
  const task = encodeURIComponent(row.task_id || "");
  switch (action) {
    case "run_again":
    case "run_now":
      return { path: `/api/tasks/${task}/run`, method: "POST", body: withReason };
    case "cancel":
      // The attempt the person was shown: a retry that started since is
      // refused rather than ended (`Request::TaskCancel::run`).
      return { path: `/api/tasks/${task}/cancel`, method: "POST", body: { ...withReason, ...(row.run_id ? { run_id: row.run_id } : {}) } };
    case "skip_next":
      // The firing the person was shown, so a schedule that moved on in the
      // meantime is refused rather than skipping a slot nobody chose.
      return { path: `/api/tasks/${task}/skip-next`, method: "POST", body: { ...withReason, ...(row.next_run_at ? { slot: row.next_run_at } : {}) } };
    case "pause_schedule":
      return { path: `/api/tasks/${task}`, method: "PATCH", body: { schedule_paused: true, ...withReason } };
    case "resume_schedule":
      return { path: `/api/tasks/${task}`, method: "PATCH", body: { schedule_paused: false, ...withReason } };
    case "answer":
      return { path: `/api/runs/${encodeURIComponent(row.run_id || "")}/answer`, method: "POST", body: { text: text || "", reason: why } };
    default:
      return null;
  }
}

/// Whether the dialog's confirm button may be pressed: an answer needs
/// both its text and a reason (the daemon refuses either missing, and
/// saying so before the round trip is kinder); everything else may go
/// without a reason.
export function actionReady(action, { reason, text } = {}) {
  if (action === "answer") return !!(text && text.trim()) && !!(reason && reason.trim());
  return true;
}

/// The sentence a confirmation dialog leads with -- what the action does
/// and what it costs, in the ticket's words (a run spends a model call; a
/// cancel is scrap; a resume never catches up).
export function actionConsequence(action, row) {
  const what = row.title ? `“${row.title}”` : "this task";
  switch (action) {
    case "run_again":
      return `Starts a new attempt of ${what} now. It spends a model call.`;
    case "run_now":
      return `Starts ${what} now, ahead of its next slot. It spends a model call; the schedule itself is unchanged.`;
    case "cancel":
      return `Cancels the running attempt of ${what}. A cancelled run counts as scrap.`;
    case "skip_next":
      return `Passes over the next firing of ${what}${row.next_run_at ? `, due ${utcStamp(row.next_run_at)}` : ""}. If a retry is queued, the retry is that firing: skipping it ends the streak and the regular slot comes back. If the schedule has moved on since this page read it, the skip is refused.`;
    case "pause_schedule":
      return `Stops the line for ${what}: nothing fires until the schedule is resumed. The schedule itself is kept.`;
    case "resume_schedule":
      return `Resumes the schedule of ${what}. The next slot is worked out from now -- slots passed while paused are not caught up.`;
    case "answer":
      return `Types your answer into the blocked run's own session and presses enter. The run stays blocked until its agent says otherwise.`;
    default:
      return "";
  }
}

// ------------------------------------------------------------------ bulk

/// The rows one bulk action would touch: exactly those among `rows` that
/// offer it, and one per task -- two exceptions of one task would
/// otherwise run it twice. The preview lists every one of them, with the
/// count, before anything is sent.
export function bulkPreview(rows, action) {
  const seen = new Set();
  const out = [];
  for (const r of rows) {
    if (!(r.actions || []).includes(action) || !r.task_id || seen.has(r.task_id)) continue;
    seen.add(r.task_id);
    out.push(r);
  }
  return { action, count: out.length, rows: out };
}

/// The bulk actions worth a button: only run again and cancel (the
/// ticket's list), and only when more than one row offers them.
export function bulkOffers(rows) {
  return ["run_again", "cancel"]
    .map((a) => bulkPreview(rows, a))
    .filter((p) => p.count > 1);
}

// ------------------------------------------------------------------- flow

/// Work in flight per scope, as stacked bars on one shared scale -- the
/// longest bar is the busiest scope, so two scopes compare at a glance.
export function flowBars(flow, inScope) {
  const rows = (flow || []).filter((f) => (inScope ? inScope(f.scope) : true));
  const totals = rows.map((f) => STAGES.reduce((n, s) => n + (f.wip[s] || 0), 0));
  const max = Math.max(1, ...totals);
  return rows.map((f, i) => {
    let x = 0;
    const segments = STAGES.filter((s) => f.wip[s]).map((s) => {
      const w = (f.wip[s] / max) * 100;
      const seg = { stage: s, n: f.wip[s], x, w };
      x += w;
      return seg;
    });
    return { ...f, total: totals[i], segments };
  });
}

/// Sessions in use against capacity. `sessions_max` is absent, never zero,
/// while `max_sessions` has no effect (config.rs), so capacity reads
/// "unknown" -- a utilisation figure over an invented ceiling would be the
/// vanity number the ticket forbids.
export function capacityText(f) {
  if (f.sessions_max === undefined || f.sessions_max === null) return `${f.sessions_in_use} in use · capacity unknown`;
  const pct = f.sessions_max ? Math.round((f.sessions_in_use / f.sessions_max) * 100) : 0;
  return `${f.sessions_in_use} of ${f.sessions_max} · ${pct}%`;
}

/// A `Figure` as text: the number when there is one, else its reason --
/// "unknown" is never drawn as zero.
export function figureText(fig, fmt) {
  if (!fig) return "—";
  if (fig.value === null || fig.value === undefined) return "—";
  return fmt(fig.value);
}

export function pct(v) {
  return `${(v * 100).toFixed(v < 0.1 ? 1 : 0)}%`;
}

// ------------------------------------------------------------ aging WIP

/// The lines for `scope`, or null for "not enough history".
export function linesFor(aging, scope) {
  const p = ((aging && aging.percentiles) || []).find((x) => x.scope === scope);
  return p && p.lines ? p.lines : null;
}

/// Stable jitter in [-0.5, 0.5) from an id, so a dot keeps its place in its
/// column across re-renders instead of hopping every refetch.
export function jitter(id) {
  let h = 2166136261;
  for (const c of String(id)) {
    h ^= c.charCodeAt(0);
    h = Math.imul(h, 16777619);
  }
  return ((h >>> 0) % 1000) / 1000 - 0.5;
}

const LOG_TICKS = [
  [10, "10s"], [60, "1m"], [600, "10m"], [3600, "1h"], [6 * 3600, "6h"],
  [86400, "1d"], [7 * 86400, "7d"], [30 * 86400, "30d"],
];

/// A log scale over seconds. Its floor is the tick at or below half the
/// smallest value, never under ten seconds -- a task that became due a
/// moment ago has an age of nearly nothing, and log(0) is not a place on a
/// chart -- so a chart of hour-long runs does not spend most of its height
/// on the seconds nobody is in.
export function logScale(values, height, pad = 0) {
  const finite = values.filter((v) => Number.isFinite(v) && v > 0);
  const least = finite.length ? Math.min(...finite) / 2 : 10;
  const floor = Math.max(10, ...LOG_TICKS.map(([v]) => v).filter((v) => v <= least));
  const top = Math.max(floor * 6, ...finite) * 1.6;
  const lo = Math.log10(floor);
  const hi = Math.log10(top);
  const y = (v) => {
    const t = (Math.log10(Math.max(floor, v)) - lo) / (hi - lo || 1);
    return pad + (1 - t) * (height - 2 * pad);
  };
  const ticks = LOG_TICKS.filter(([v]) => v >= floor && v <= top).map(([v, label]) => ({ v, label, y: y(v) }));
  return { y, ticks, top, floor };
}

/// Label positions for lines drawn at `ys` (any order), each at least
/// `gap` apart, pushed apart from the middle out so they neither overlap
/// nor stray far from their own line. Returned in the order given.
export function spreadLabels(ys, gap) {
  const order = ys.map((y, i) => ({ y, i })).sort((a, b) => a.y - b.y);
  const placed = order.map((o) => o.y);
  for (let k = 1; k < placed.length; k++) placed[k] = Math.max(placed[k], placed[k - 1] + gap);
  // Centre the block on where the lines really are, so a tight cluster
  // spreads both ways rather than only downward.
  const shift = placed.length ? (placed.reduce((a, b) => a + b, 0) - order.reduce((a, o) => a + o.y, 0)) / placed.length : 0;
  const out = new Array(ys.length);
  order.forEach((o, k) => { out[o.i] = placed[k] - shift; });
  return out;
}

/// Vacanti's Aging WIP chart for one scope: stage on x, age on a log y,
/// the scope's p50/p70/p85/p95 as dashed lines, one dot per item coloured
/// by the pace the daemon judged -- which may be against the task's own
/// history rather than these scope lines, and the dot's title says whose.
///
/// An item carries its age as of the report, not a start time, so
/// `elapsedS` -- how long ago the report was generated -- is added to keep
/// a dot rising between two fetches rather than frozen at the last one.
export function agingGeometry(items, lines, elapsedS = 0, { width = 600, height = 240 } = {}) {
  const pad = 14;
  const colW = width / STAGES.length;
  const ages = items.map((it) => it.age_s + Math.max(0, elapsedS));
  const lineVals = lines ? [lines.p50, lines.p70, lines.p85, lines.p95] : [];
  const scale = logScale([...ages, ...lineVals], height, pad);
  const dots = items.map((it, i) => {
    const col = STAGES.indexOf(it.stage);
    const cx = (Math.max(0, col) + 0.5 + jitter(it.run_id || it.task_id) * 0.6) * colW;
    return { ...it, age: ages[i], cx, cy: scale.y(ages[i]), paceClass: it.pace ? `pace-${it.pace}` : "pace-none" };
  });
  const pLines = lines
    ? [["p50", lines.p50], ["p70", lines.p70], ["p85", lines.p85], ["p95", lines.p95]].map(([name, v]) => ({ name, v, y: scale.y(v) }))
    : [];
  const labelY = spreadLabels(pLines.map((l) => l.y), 11);
  pLines.forEach((l, i) => { l.labelY = labelY[i]; });
  const columns = STAGES.map((s, i) => ({ stage: s, x: i * colW, w: colW, cx: (i + 0.5) * colW }));
  return { width, height, dots, lines: pLines, ticks: scale.ticks, columns };
}

/// The chart's items grouped by scope -- one chart each, since each scope
/// has its own percentile lines and one set of lines over another scope's
/// dots would be a comparison nobody asked for.
export function agingByScope(aging, inScope) {
  const by = new Map();
  for (const it of (aging && aging.items) || []) {
    if (inScope && !inScope(it.scope)) continue;
    if (!by.has(it.scope)) by.set(it.scope, []);
    by.get(it.scope).push(it);
  }
  return [...by.entries()].map(([scope, items]) => ({ scope, items, lines: linesFor(aging, scope) }));
}

export function basisText(basis) {
  return {
    task: "against this task's own finished runs",
    scope: "against its scope's finished runs (the task has fewer than 5)",
    not_enough_history: "not enough history -- fewer than 5 finished runs",
    not_paced: "not paced -- no run yet to compare",
  }[basis] || words(basis);
}

// ------------------------------------------------------------------ health

/// The small multiples, each a line over the window's 24-hour steps with
/// the window before it as a ghost. A rate step with nothing finished is a
/// gap, not a zero -- `null` breaks the line.
export const MULTIPLES = [
  { id: "throughput", title: "Throughput / day", fig: "throughput_day", fmt: (v) => v.toFixed(1), better: "higher", of: (d) => d.finished },
  { id: "first_pass", title: "First-pass yield", fig: "first_pass_yield", fmt: pct, better: "higher", of: (d) => (d.finished ? d.first_pass / d.finished : null) },
  { id: "rework", title: "Rework rate", fig: "rework_rate", fmt: pct, better: "lower", of: (d) => (d.finished ? d.reworked / d.finished : null) },
  { id: "scrap", title: "Scrap rate", fig: "scrap_rate", fmt: pct, better: "lower", of: (d) => (d.finished ? d.scrapped / d.finished : null) },
  { id: "fail", title: "Fail rate", fig: "fail_rate", fmt: pct, better: "lower", of: (d) => (d.finished ? d.failed / d.finished : null) },
];

/// Figures with no per-step series of their own -- percentiles of a few
/// runs a day are noise -- shown as a number against the previous window's.
export const TILES = [
  { id: "cycle_p50", title: "Cycle time p50", fig: "cycle_p50", fmt: fmtAge, better: "lower" },
  { id: "cycle_p85", title: "Cycle time p85", fig: "cycle_p85", fmt: fmtAge, better: "lower" },
  { id: "recover_p50", title: "Time to recover p50", fig: "recover_p50", fmt: fmtAge, better: "lower" },
  { id: "wait_p95", title: "Queue wait p95", fig: "queue_wait_p95", fmt: fmtAge, better: "lower" },
  { id: "interventions", title: "Interventions / 100 runs", fig: "interventions_per_100", fmt: (v) => v.toFixed(1), better: "lower" },
];

/// Which way a figure moved against the previous window, and whether that
/// is the good way. `flat` inside 5% either side, so noise is not a trend.
export function trend(cur, prev, better) {
  const a = cur && cur.value;
  const b = prev && prev.value;
  if (a === null || a === undefined || b === null || b === undefined) return { dir: "none", tone: "flat" };
  const scale = Math.max(Math.abs(a), Math.abs(b), 1e-9);
  if (Math.abs(a - b) / scale < 0.05) return { dir: "flat", tone: "flat" };
  const up = a > b;
  const good = better === "higher" ? up : !up;
  return { dir: up ? "up" : "down", tone: good ? "up" : "down" };
}

/// One small multiple's two polylines on a shared y scale. Points are
/// `[x, y]` pairs split into runs at every null, so a gap stays a gap.
export function multipleGeometry(current, previous, of, { width = 160, height = 44 } = {}) {
  const cur = (current || []).map(of);
  const prev = (previous || []).map(of);
  const vals = [...cur, ...prev].filter((v) => v !== null && v !== undefined);
  const max = Math.max(...vals, 0) || 1;
  const n = Math.max(cur.length, prev.length);
  const step = n > 1 ? width / (n - 1) : 0;
  const pad = 3;
  const y = (v) => pad + (1 - v / max) * (height - 2 * pad);
  const segs = (series) => {
    const out = [];
    let run = [];
    series.forEach((v, i) => {
      if (v === null || v === undefined) {
        if (run.length) out.push(run);
        run = [];
      } else run.push([+(i * step).toFixed(1), +y(v).toFixed(1)]);
    });
    if (run.length) out.push(run);
    return out;
  };
  return { width, height, max, current: segs(cur), previous: segs(prev), empty: !vals.length };
}

/// Failures and cancels by why, largest first, with the ones from before
/// `fail_kind` existed kept apart as `unclassified` -- never folded into a
/// kind they might not have been.
export function failKindRows(health) {
  const rows = Object.entries((health && health.fail_by_kind) || {})
    .map(([kind, n]) => ({ kind, label: failKindLabel(kind), n }))
    .sort((a, b) => b.n - a.n || a.kind.localeCompare(b.kind));
  if (health && health.unclassified) rows.push({ kind: "unclassified", label: "unclassified (before fail kinds were recorded)", n: health.unclassified });
  const max = Math.max(1, ...rows.map((r) => r.n));
  return rows.map((r) => ({ ...r, w: (r.n / max) * 100 }));
}

/// The cycle-time scatter: every run that ended done in the window, by
/// when it ended (x) and how long it took (log y), with the window's
/// p50/p85 as lines -- the drill-down behind the two cycle-time tiles.
export function scatterGeometry(finishedRuns, window, health, { width = 600, height = 200 } = {}) {
  const pad = 12;
  const done = (finishedRuns || []).filter((r) => r.cycle_s !== undefined && r.cycle_s !== null);
  const from = Date.parse(window.from);
  const to = Date.parse(window.to);
  const span = Math.max(1, to - from);
  const lines = [["p50", health && health.cycle_p50], ["p85", health && health.cycle_p85]]
    .filter(([, f]) => f && f.value !== null && f.value !== undefined)
    .map(([name, f]) => ({ name, v: f.value }));
  const scale = logScale([...done.map((r) => r.cycle_s), ...lines.map((l) => l.v)], height, pad);
  return {
    width,
    height,
    ticks: scale.ticks,
    lines: (() => {
      const placed = lines.map((l) => ({ ...l, y: scale.y(l.v) }));
      const labelY = spreadLabels(placed.map((l) => l.y), 11);
      return placed.map((l, i) => ({ ...l, labelY: labelY[i] }));
    })(),
    dots: done.map((r) => ({ ...r, cx: ((Date.parse(r.ended_at) - from) / span) * width, cy: scale.y(r.cycle_s) })),
    empty: !done.length,
  };
}

/// The cumulative flow diagram over the window's steps: finished so far at
/// the bottom, then what stood in progress, then what stood waiting, as
/// three stacked areas. The finished band counts from the window's start,
/// so it says how much left the line in this window, not ever.
export function cfdGeometry(days, { width = 600, height = 180 } = {}) {
  const list = days || [];
  let cum = 0;
  const stacks = list.map((d) => {
    cum += d.finished;
    return { to: d.to, done: cum, progress: cum + d.in_progress, waiting: cum + d.in_progress + d.waiting };
  });
  const max = Math.max(1, ...stacks.map((s) => s.waiting));
  const n = stacks.length;
  const x = (i) => (n > 1 ? (i / (n - 1)) * width : width / 2);
  const y = (v) => height - (v / max) * height;
  const area = (top, bottom) => {
    if (!n) return "";
    const upper = stacks.map((s, i) => `${x(i).toFixed(1)},${y(top(s)).toFixed(1)}`);
    const lower = stacks.map((s, i) => `${x(i).toFixed(1)},${y(bottom(s)).toFixed(1)}`).reverse();
    return [...upper, ...lower].join(" ");
  };
  return {
    width,
    height,
    max,
    stacks,
    bands: [
      { id: "done", label: "finished (cumulative)", points: area((s) => s.done, () => 0) },
      { id: "progress", label: "in progress", points: area((s) => s.progress, (s) => s.done) },
      { id: "waiting", label: "waiting", points: area((s) => s.waiting, (s) => s.progress) },
    ],
    empty: !n || stacks.every((s) => s.waiting === 0),
  };
}

// --------------------------------------------------------------- schedules

const SCHEDULE_ORDER = { late: 0, missed: 1, paused: 2, due: 3 };

/// Late first, then missed, then paused, then the rest by their next slot.
export function scheduleRows(schedules, inScope) {
  return (schedules || [])
    .filter((s) => (inScope ? inScope(s.scope) : true))
    .map((s) => ({
      ...s,
      next: s.next_run_at ? utcStamp(s.next_run_at) : "—",
      zone: s.timezone || "UTC",
    }))
    .sort((a, b) => (SCHEDULE_ORDER[a.state] ?? 9) - (SCHEDULE_ORDER[b.state] ?? 9) || String(a.next_run_at || "").localeCompare(String(b.next_run_at || "")) || a.title.localeCompare(b.title));
}

/// What a schedule row offers, by its state -- the same actions the daemon
/// attaches to a late or missed slot's exception, and pause/resume for the
/// rest. A paused schedule has no slot coming, so nothing to run early or
/// skip; it can only be resumed.
export function scheduleActions(row) {
  if (row.state === "paused") return ["resume_schedule"];
  return ["run_now", "skip_next", "pause_schedule"];
}

/// Workflow nodes that are blocked, and so hold back the rest of their
/// DAG -- read off the task list every page already has.
export function blockedWorkflowNodes(tasks, inScope) {
  return (tasks || [])
    .filter((t) => t.status === "blocked" && t.workflow_origin && (inScope ? inScope(t.scope) : true))
    .sort((a, b) => String(a.updated_at).localeCompare(String(b.updated_at)));
}

// --------------------------------------------------------------- journal
//
// The task modal's journal, for the entries the Operations actions write.
// The message already says who asked and why (`operations.rs`'s `Asked`);
// what the modal adds is a readable kind and a mark that a person, not
// the daemon or the agent, did it.

export const ENTRY_KINDS = {
  run_requested: "run requested",
  cancel_requested: "cancel requested",
  slot_skipped: "slot skipped",
  schedule_skipped: "slots missed",
  schedule_paused: "schedule paused",
  schedule_resumed: "schedule resumed",
  schedule_pause_cleared: "pause cleared",
  answer: "answered",
  answer_unsent: "answer not sent",
  closed: "closed",
  reopened: "reopened",
  blocked_on_failure: "blocked on failure",
  migrated: "moved to blocked",
};

export function entryKindLabel(kind) {
  return ENTRY_KINDS[kind] || kind;
}

/// A class for an entry's line: an action a person or agent asked for is
/// `ask`; a slot the daemon passed over on its own is `missed`; an answer
/// typed whose enter could not be pressed is `fault` -- the agent may be
/// sitting on half a line.
export function entryTone(entry) {
  if (entry.kind === "schedule_skipped") return "missed";
  if (entry.kind === "answer_unsent") return "fault";
  // The daemon's own lines about a failure (#122), not somebody's ask.
  if (entry.kind === "blocked_on_failure") return "fault";
  if (entry.kind === "migrated") return "";
  if (ENTRY_KINDS[entry.kind]) return "ask";
  return "";
}
