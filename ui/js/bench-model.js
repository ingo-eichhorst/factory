//! Pure helpers shared by the Benchmarks tab's three segments -- Datasets,
//! Runs and the shell that switches between them and Configurations.
//!
//! `datasets.js` and `bench-runs.js` import `modal.js` (a scrim for their
//! forms and the Run… modal) and `tasks.js` (to open an attempt's task), and
//! `modal.js` touches `document` at import time (`document.addEventListener`
//! for Escape) -- a Node test has no DOM for that. So every function a test
//! needs lives here instead, importing nothing that is not itself safe to
//! import from Node: the same boundary `workflow-model.js`/`workflow-graph.js`
//! already keep for Workflows, and the reason `benchmarks.js` -- imported
//! directly by `improvement.test.js` -- must never import `datasets.js` or
//! `bench-runs.js` itself.

import { inScope } from "./scopes.js";

// ------------------------------------------------------------------ routing
//
// `#<scope>/imp/benchmarks/datasets/<name>`, `.../runs/<id>` and
// `.../configurations` -- the segmented control's own tail, read and written
// the same way `workflowRouteTail`/`readWorkflowRouteTail` handle a workflow
// id and an optional run. A dataset name matches `[a-z0-9][a-z0-9-]*`, which
// legally spells `task` -- app.js's `splitTail` cuts a view's tail at the
// first segment equal to its `MODAL` marker ("task"), so a dataset actually
// named `task` needs the same escape `noteTail` uses in knowledge.js.

const RESERVED = "task";

function escSeg(s) {
  return s === RESERVED ? `${RESERVED}~` : s;
}

function unescSeg(s) {
  return s === `${RESERVED}~` ? RESERVED : s;
}

/// Pure: the current segment and selection to the tail segments that name
/// it. `dataset`/`run` are ignored for the segment they do not belong to, so
/// a caller can pass both selections unconditionally.
export function benchmarksTail(segment, dataset, run) {
  if (segment === "runs") return run ? ["runs", run] : ["runs"];
  if (segment === "configurations") return ["configurations"];
  return dataset ? ["datasets", escSeg(dataset)] : ["datasets"];
}

/// The inverse: a tail to `{segment, dataset, run}`. An empty tail, or one
/// naming neither "runs" nor "configurations", is the default -- Datasets is
/// first in the segmented control and the newer of the two front doors this
/// issue adds, so a bare `#scope/imp/benchmarks` link lands there rather than
/// on the unchanged Configurations cards.
export function readBenchmarksTail(tail) {
  const [seg, id] = tail || [];
  if (seg === "runs") return { segment: "runs", dataset: null, run: id || null };
  if (seg === "configurations") return { segment: "configurations", dataset: null, run: null };
  return { segment: "datasets", dataset: id ? unescSeg(id) : null, run: null };
}

// ----------------------------------------------------------------- datasets

/// How many of a dataset's cases have no gate and so run `unverified` --
/// `DatasetSummary` carries `cases` and `gated` but not this directly.
export function ungated(summary) {
  return Math.max(0, (summary && summary.cases || 0) - (summary && summary.gated || 0));
}

/// Narrows a dataset's cases to the rail's inclusive scope selection. The
/// dataset list itself stays company-wide (the issue is explicit about
/// that); this is for the case table of one selected dataset.
export function visibleCases(cases, contains = inScope) {
  return (cases || []).filter((c) => contains(c.scope));
}

const IMPORT_EXTENSIONS = { jsonl: "jsonl", json: "json", yaml: "yaml", yml: "yml", csv: "csv" };

/// The import format `factory_core::dataset::import` expects, read off a
/// file name's extension -- `.jsonl`, `.json`, `.yaml`/`.yml`, or `.csv`.
/// `null` for anything else, which the caller refuses before ever reading
/// the file.
export function importFormat(filename) {
  const m = /\.([^.]+)$/.exec(String(filename || "").toLowerCase());
  return (m && IMPORT_EXTENSIONS[m[1]]) || null;
}

/// The daemon refuses a bad import all-or-nothing, as one `BadRequest`
/// message joining every `ImportProblem::to_string()` with `"; "`
/// (`dataset.rs`'s importers, via `datasets.rs`) -- there is no structured
/// list on the wire, only this one string, so splitting it back into rows is
/// the whole of what a UI can do to name each line or row and field. Each
/// `ImportProblem`'s own `Display` already reads `line N, field: detail` or
/// `line N: detail`, so a row here already carries what the issue asks for.
export function splitImportErrors(message) {
  return String(message || "")
    .split("; ")
    .map((s) => s.trim())
    .filter(Boolean);
}

/// `^[a-z0-9][a-z0-9-]*$` -- a dataset's `name` and a case's `id`, mirrored
/// from `factory_core::dataset::is_slug` for client-side form feedback. The
/// daemon is still the one source of truth; this only saves a round trip on
/// an obviously bad value.
export function isSlug(s) {
  return /^[a-z0-9][a-z0-9-]*$/.test(String(s || ""));
}

/// Every agent name the Run… modal can offer, from the configurations tab's
/// own inventory and the roster -- the issue asks for both. `bench.run` only
/// ever keeps the trailing agent name (it is resolved fresh in each case's
/// own scope), so this collects names, not scope-qualified paths.
export function agentChoices(configurations, scopes) {
  const names = new Set();
  for (const c of configurations || []) {
    for (const a of c.agents || []) names.add(a.agent);
  }
  for (const s of scopes || []) {
    for (const a of s.agents || []) names.add(a.name);
  }
  return [...names].sort();
}

/// The "From tasks" picker's own filter: recorded tasks narrowed by scope and
/// status, both optional. Pure so the picker's matching logic can be tested
/// without a fetch or a DOM.
export function filterTasksForPicker(tasks, scope, status) {
  return (tasks || []).filter(
    (t) => (!scope || t.scope === scope) && (!status || t.status === status),
  );
}

// --------------------------------------------------------------- bench runs

/// `settled/total`, the same definition the CLI's `bench_run_line` uses
/// (`main.rs`), so the list and the daemon's own text never disagree about
/// what "settled" means.
export function progress(attempts) {
  const list = attempts || [];
  return { settled: list.filter((a) => a.verdict != null).length, total: list.length };
}

/// pass/fail/unverified/skipped/cancelled/error come straight off the
/// verdict; an attempt with no verdict yet is `running` once it has a task
/// and `pending` before it does. `bench/engine.rs` writes `verdict` and
/// `ended_at` in the same step (`judge_bench_attempt`), so there is no
/// separate "settled but not yet judged" state to read apart from `running`
/// today -- see the issue's note on asynchronous judging landing later.
export function attemptState(attempt) {
  if (attempt && attempt.verdict) return attempt.verdict;
  return attempt && attempt.task_id ? "running" : "pending";
}

/// One column per result row, in the order the daemon already sorted them
/// (`bench::aggregate`, by label then hash) -- the results table and the
/// attempts matrix share this array so the two can never disagree about
/// which configurations exist or in what order.
export function matrixColumns(results) {
  return (results || []).map((r) => ({ config_hash: r.config_hash, label: r.label }));
}

/// `attempts` grouped by case id, then by `config_hash` -- an attempt whose
/// agent never resolved a configuration lands under the empty-string hash,
/// the same row `aggregate` gives it in the results table. Plain nested
/// objects rather than a `Map`, so a test can assert on it with `deepEqual`.
export function attemptsByCell(attempts) {
  const byCase = {};
  for (const a of attempts || []) {
    const hash = (a.config && a.config.config_hash) || "";
    if (!byCase[a.case_id]) byCase[a.case_id] = {};
    if (!byCase[a.case_id][hash]) byCase[a.case_id][hash] = [];
    byCase[a.case_id][hash].push(a);
  }
  return byCase;
}

/// Resolve rate against mean wall-clock, one point per configuration that
/// has both -- a configuration with nothing gated (`resolve_rate: null`) or
/// no attempt that ever reported a wall-clock time draws no point rather
/// than a fabricated zero.
export function scatterPoints(results) {
  return (results || [])
    .filter((r) => r.resolve_rate != null && r.mean_wall_clock_seconds != null)
    .map((r) => ({
      x: r.mean_wall_clock_seconds,
      y: r.resolve_rate,
      label: r.label,
      config_hash: r.config_hash,
    }));
}
