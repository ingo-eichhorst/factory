//! L5 Improvement, Benchmarks tab, Runs segment: what came out of a bench
//! run -- a results table per configuration, an attempts matrix of case ×
//! configuration, and a small scatter of resolve rate against mean
//! wall-clock. Live-updates on the `bench_run_updated` socket event, the
//! same way `workflows.js` follows `workflow_run_updated`.
//!
//! The rail narrows the attempts matrix's own rows to the selected scope,
//! following the v1 `visibleConfigurations` pattern -- but never the results
//! table, which stays the daemon's own aggregation (`bench::aggregate`) over
//! every case in the run. Re-deriving a narrower aggregate here would risk a
//! number that disagrees with the one the daemon computed for the same run;
//! `bench-run-scope-note` says so.
//!
//! Like `datasets.js`, this file freely imports `modal.js`/`tasks.js` and so
//! is never imported by a Node test; the pure shaping it renders from lives
//! in `bench-model.js`.

import { $, api, esc, shortSpan, state, statusBadge } from "./core.js";
import { writeHash } from "./scopes.js";
import { openTask } from "./tasks.js";
import { attemptState, attemptsByCell, matrixColumns, progress, scatterPoints, visibleCases } from "./bench-model.js";

const FIXED_NOTE =
  "A score without its configuration is not a measurement. Resolve rate counts gated attempts only. An " +
  "unverified attempt is the agent's own word: shown, never averaged in. Cost and tokens are not recorded yet, " +
  "and no configuration is pinned.";

const SCOPE_NOTE =
  "The results table above covers every case in this run; the rail beside this page narrows only the attempts " +
  "matrix below, never the aggregated results.";

// ------------------------------------------------------------------ loading

/// Sets `state.benchRuns` (or `state.benchRunsError`, never both), settles
/// the selection against the fresh list, and loads the selected run's own
/// detail, if any -- the same "settle then correct" shape `loadDatasets`
/// uses in `datasets.js`.
export async function loadBenchRuns() {
  try {
    state.benchRuns = await api("/api/bench/runs");
    state.benchRunsError = null;
  } catch (error) {
    state.benchRuns = null;
    state.benchRunsError = error.message;
  }
  const list = state.benchRunsError ? [] : (state.benchRuns && state.benchRuns.runs) || [];
  if (state.benchRunId && !list.some((r) => r.id === state.benchRunId)) {
    state.benchRunId = null;
    state.benchRun = null;
    state.benchRunError = null;
    writeHash(true);
  }
  renderBenchRunsSegment();
  if (state.benchRunId) await loadBenchRunDetail(state.benchRunId);
}

/// A sequence number rather than a boolean busy flag: a live
/// `bench_run_updated` event can fire faster than one fetch settles, and the
/// point is to keep only the freshest answer, not to serialize every call.
let detailSeq = 0;

export async function loadBenchRunDetail(id) {
  const seq = ++detailSeq;
  try {
    const data = await api(`/api/bench/runs/${encodeURIComponent(id)}`);
    if (seq !== detailSeq) return;
    state.benchRun = data;
    state.benchRunError = null;
  } catch (error) {
    if (seq !== detailSeq) return;
    state.benchRun = null;
    state.benchRunError = error.message;
  }
  renderBenchRunsSegment();
}

function selectBenchRun(id) {
  if (id === state.benchRunId) return;
  state.benchRunId = id;
  state.benchRun = null;
  state.benchRunError = null;
  renderBenchRunsSegment();
  writeHash();
  loadBenchRunDetail(id);
}

// ------------------------------------------------------------------ render

function benchRunRow(r) {
  const { settled, total } = progress(r.attempts);
  const on = r.id === state.benchRunId ? ` aria-current="true"` : "";
  return `<tr class="row" data-id="${esc(r.id)}"${on}>
    <td>${esc(r.dataset)}@${esc(r.dataset_revision)}</td>
    <td class="sub">${esc((r.agents || []).join(", "))}</td>
    <td class="sub">${esc(new Date(r.started_at).toLocaleString())}</td>
    <td>${statusBadge(r.status)}</td>
    <td class="sub">${settled}/${total} settled</td>
  </tr>`;
}

function resultRow(r) {
  const rate = r.resolve_rate == null ? `<span class="sub">not gated</span>` : `${(r.resolve_rate * 100).toFixed(1)}%`;
  const wall = r.mean_wall_clock_seconds == null ? `<span class="sub">—</span>` : shortSpan(r.mean_wall_clock_seconds);
  return `<tr>
    <td>${esc(r.label)}</td>
    <td>${rate}</td>
    <td>${esc(r.pass)}</td>
    <td>${esc(r.fail)}</td>
    <td>${esc(r.unverified)}</td>
    <td>${esc(r.skipped)}</td>
    <td>${wall}</td>
    <td><span class="sub">not recorded</span></td>
  </tr>`;
}

/// A verdict chip: a plain `<span>` for an attempt with no task yet
/// (`pending`, or `skipped` before ever dispatching -- an unresolved base or
/// a missing agent), a clickable `<button>` -- the app's own task deep link,
/// `openTask` -- for every other state, since every attempt past `pending`
/// has a task behind it. `attemptState` needs the task's own status to tell
/// `running` and `judging` apart, read from `state.tasks` -- the same map
/// the WebSocket snapshot already keeps current for every task, bench
/// attempts included.
function chip(attempt) {
  const taskStatus = attempt.task_id ? state.tasks.get(attempt.task_id)?.status : undefined;
  const v = attemptState(attempt, taskStatus);
  const why = attempt.reason ? ` -- ${attempt.reason}` : "";
  if (!attempt.task_id) {
    return `<span class="badge v-${esc(v)}" title="attempt ${esc(attempt.attempt)}${esc(why)}">${esc(v)}</span>`;
  }
  return `<button type="button" class="badge v-${esc(v)}" data-open-task="${esc(attempt.task_id)}"
    title="attempt ${esc(attempt.attempt)} -- open its task${esc(why)}">${esc(v)}</button>`;
}

function renderScatter(results) {
  const svg = $("bench-scatter");
  const caption = $("bench-scatter-caption");
  if (!svg) return;
  const points = scatterPoints(results);
  if (caption) {
    caption.textContent = points.length
      ? "Resolve rate (gated attempts only) against mean wall-clock. Cost is not recorded and cannot be plotted."
      : "Nothing to plot yet -- no configuration in this run has both a resolve rate and a timed attempt.";
  }
  if (!points.length) {
    svg.innerHTML = "";
    return;
  }
  const W = 420;
  const H = 220;
  const padL = 42;
  const padR = 14;
  const padT = 12;
  const padB = 26;
  const maxX = Math.max(...points.map((p) => p.x), 1);
  const px = (v) => padL + (v / maxX) * (W - padL - padR);
  const py = (v) => H - padB - v * (H - padT - padB);
  const axes = `
    <line x1="${padL}" y1="${padT}" x2="${padL}" y2="${H - padB}" class="bench-scatter-axis"></line>
    <line x1="${padL}" y1="${H - padB}" x2="${W - padR}" y2="${H - padB}" class="bench-scatter-axis"></line>
    <text x="4" y="${padT + 8}" class="bench-scatter-label">100%</text>
    <text x="4" y="${H - padB + 4}" class="bench-scatter-label">0%</text>
    <text x="${padL}" y="${H - 8}" class="bench-scatter-label">0s</text>
    <text x="${W - padR - 30}" y="${H - 8}" class="bench-scatter-label">${esc(shortSpan(maxX))}</text>`;
  const dots = points
    .map(
      (p) => `<circle cx="${px(p.x).toFixed(1)}" cy="${py(p.y).toFixed(1)}" r="5" class="bench-scatter-point">
        <title>${esc(p.label)} — ${(p.y * 100).toFixed(1)}% · ${esc(shortSpan(p.x))}</title>
      </circle>`,
    )
    .join("");
  svg.innerHTML = axes + dots;
}

function renderBenchRunDetail() {
  const el = $("bench-run-detail");
  if (!el) return;
  if (!state.benchRunId) {
    el.hidden = true;
    return;
  }
  el.hidden = false;

  // Guard against a stale answer even if one somehow survives the clear in
  // `readBenchTail` (an overlapping fetch from before the selection changed)
  // -- `detailSeq` only keeps the newest *call* winning, not the newest
  // *selection*, so a cached run is only ever trusted for the id it actually
  // answers.
  const data = state.benchRunError || !state.benchRun || state.benchRun.run.id !== state.benchRunId
    ? null
    : state.benchRun;
  if (!data) {
    $("bench-run-title").textContent = state.benchRunId;
    $("bench-run-status").textContent = state.benchRunError || "loading…";
    $("bench-results").innerHTML = "";
    $("bench-matrix-head").innerHTML = "<th>Case</th>";
    $("bench-matrix-body").innerHTML = "";
    $("no-bench-attempts").hidden = true;
    $("bench-scatter").innerHTML = "";
    $("bench-scatter-caption").textContent = "";
    $("bench-run-cancel").disabled = true;
    $("bench-run-clean").disabled = true;
    return;
  }
  const { run, results } = data;
  $("bench-run-title").textContent = `${run.dataset}@${run.dataset_revision}`;
  const { settled, total } = progress(run.attempts);
  $("bench-run-status").innerHTML =
    `${statusBadge(run.status)} <span class="sub">${settled}/${total} settled · agents ${esc((run.agents || []).join(", "))}</span>`;

  $("bench-run-cancel").disabled = run.status !== "running";
  $("bench-run-clean").disabled = run.status === "running";

  $("bench-results").innerHTML = results.map(resultRow).join("");

  const columns = matrixColumns(results);
  $("bench-matrix-head").innerHTML = `<th>Case</th>${columns.map((c) => `<th>${esc(c.label)}</th>`).join("")}`;
  const cells = attemptsByCell(run.attempts);
  const cases = visibleCases(run.cases);
  $("bench-matrix-body").innerHTML = cases
    .map(
      (c) => `<tr>
        <td><div class="title">${esc(c.id)}</div><div class="sub">${esc(c.title)}</div></td>
        ${columns
          .map((col) => {
            const attempts = (cells[c.id] && cells[c.id][col.config_hash]) || [];
            return `<td>${attempts.length ? attempts.map(chip).join(" ") : `<span class="sub">—</span>`}</td>`;
          })
          .join("")}
      </tr>`,
    )
    .join("");
  for (const b of $("bench-matrix-body").querySelectorAll("[data-open-task]")) {
    b.onclick = () => openTask(b.dataset.openTask);
  }
  $("no-bench-attempts").hidden = (run.attempts || []).length !== 0;

  renderScatter(results);
}

export function renderBenchRunsSegment() {
  const note = $("bench-runs-note");
  if (note) note.textContent = FIXED_NOTE;
  const scopeNote = $("bench-run-scope-note");
  if (scopeNote) scopeNote.textContent = SCOPE_NOTE;

  const failed = $("bench-runs-error");
  if (failed) {
    failed.textContent = state.benchRunsError || "";
    failed.hidden = !state.benchRunsError;
  }

  const list = state.benchRunsError ? [] : (state.benchRuns && state.benchRuns.runs) || [];
  const rows = $("bench-run-rows");
  if (rows) {
    rows.innerHTML = list.map(benchRunRow).join("");
    for (const tr of rows.querySelectorAll("tr[data-id]")) {
      tr.onclick = () => selectBenchRun(tr.dataset.id);
    }
  }
  const empty = $("no-bench-runs");
  if (empty) empty.hidden = list.length !== 0 || !!state.benchRunsError;

  renderBenchRunDetail();
}

// ------------------------------------------------------------------ actions

async function cancelRun(id) {
  try {
    await api(`/api/bench/runs/${encodeURIComponent(id)}/cancel`, { method: "POST" });
    await loadBenchRunDetail(id);
    await loadBenchRuns();
  } catch (e) {
    alert(e.message);
  }
}

async function cleanRun(id) {
  if (!confirm("Remove this run's worktrees and branches? This cannot be undone.")) return;
  try {
    await api(`/api/bench/runs/${encodeURIComponent(id)}/clean`, { method: "POST" });
    await loadBenchRunDetail(id);
  } catch (e) {
    alert(e.message);
  }
}

export function wireBenchRuns() {
  $("bench-run-cancel").onclick = () => {
    if (state.benchRunId) cancelRun(state.benchRunId);
  };
  $("bench-run-clean").onclick = () => {
    if (state.benchRunId) cleanRun(state.benchRunId);
  };
}

// ------------------------------------------------------------------ events

/// `Event::BenchRunUpdated` carries only the run, never `results` -- unlike
/// the response to a direct fetch, which pairs the run with
/// `bench::aggregate`'s own output (`Payload::BenchRun`). So the list updates
/// straight from the event, cheaply, but the selected run's results are
/// refetched rather than re-derived here, the same call `loadBenchRunDetail`
/// already makes -- see this file's header for why that beats porting
/// `aggregate` into JS.
export function acceptBenchRunEvent(run) {
  if (state.benchRuns && state.benchRuns.runs) {
    const list = state.benchRuns.runs;
    const i = list.findIndex((r) => r.id === run.id);
    if (i >= 0) list[i] = run;
    else list.unshift(run);
  }
  renderBenchRunsSegment();
  if (state.benchRunId === run.id) loadBenchRunDetail(run.id);
}
