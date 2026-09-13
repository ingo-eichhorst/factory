import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  agentChoices,
  attemptsByCell,
  attemptState,
  benchmarksTail,
  filterTasksForPicker,
  importFormat,
  isGitRevish,
  isSlug,
  matrixColumns,
  progress,
  readBenchmarksTail,
  scatterPoints,
  splitImportErrors,
  ungated,
  visibleCases,
} from "../js/bench-model.js";
import { readBenchTail } from "../js/benchmarks.js";
import { state } from "../js/core.js";

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const app = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");

// --------------------------------------------------------------- the segments

test("the Benchmarks bar has a Datasets/Runs/Configurations segmented control, in that order", () => {
  const seg = /<div class="seg" id="bench-seg"[^>]*>([\s\S]*?)<\/div>/.exec(page);
  assert.ok(seg, "bench-seg control must exist");
  const datasets = seg[1].indexOf('data-seg="datasets"');
  const runs = seg[1].indexOf('data-seg="runs"');
  const configurations = seg[1].indexOf('data-seg="configurations"');
  assert.ok(datasets >= 0 && runs >= 0 && configurations >= 0);
  assert.ok(datasets < runs && runs < configurations, "segments must read Datasets, Runs, Configurations");
  assert.match(seg[1], /data-seg="datasets"\s+class="on"/, "Datasets is the default segment");
});

test("each segment's pane exists, and only the default segment starts visible", () => {
  assert.match(page, /id="bench-pane-datasets">/);
  assert.match(page, /id="bench-pane-runs"\s+hidden>/);
  assert.match(page, /id="bench-pane-configurations"\s+hidden>/);
});

test("the datasets and bench-runs segments each carry their own error element, hidden by default", () => {
  assert.match(page, /id="datasets-error"[^>]*hidden/);
  assert.match(page, /id="bench-runs-error"[^>]*hidden/);
});

test("tab order is unchanged: Benchmarks still follows Secrets in LEVEL_VIEWS.imp", () => {
  assert.match(app, /imp: \["benchmarks", "knowledge"\]/);
});

// ------------------------------------------------------------------- routing

test("benchmarksTail/readBenchmarksTail round-trip every segment", () => {
  assert.deepEqual(readBenchmarksTail(benchmarksTail("datasets", "registration", null)), {
    segment: "datasets",
    dataset: "registration",
    run: null,
  });
  assert.deepEqual(readBenchmarksTail(benchmarksTail("runs", null, "r1")), {
    segment: "runs",
    dataset: null,
    run: "r1",
  });
  assert.deepEqual(readBenchmarksTail(benchmarksTail("configurations", null, null)), {
    segment: "configurations",
    dataset: null,
    run: null,
  });
});

test("an empty tail defaults to the Datasets segment with nothing selected", () => {
  assert.deepEqual(readBenchmarksTail([]), { segment: "datasets", dataset: null, run: null });
  assert.deepEqual(readBenchmarksTail(undefined), { segment: "datasets", dataset: null, run: null });
});

test("a dataset legally named 'task' survives the tail round trip and the MODAL split", () => {
  for (const name of ["task", "operations-task", "task-2"]) {
    const tail = benchmarksTail("datasets", name, null);
    assert.ok(!tail.includes("task"), `tail for ${JSON.stringify(name)} must not contain a bare "task" segment`);
    assert.deepEqual(readBenchmarksTail(tail), { segment: "datasets", dataset: name, run: null });
  }
});

test("readBenchTail drops a cached dataset answer whose name no longer matches the tail", () => {
  // Back/forward is hash-driven, in-tab navigation -- the one path that
  // never runs `selectDataset` (that only fires from a click, and clears
  // the cached answer itself). Without this, `renderDatasetDetail` would
  // paint the *previous* selection's payload under the new name's header
  // until the new fetch resolves.
  state.benchDatasetName = "alpha";
  state.dataset = { dataset: { name: "alpha", revision: 1, cases: [] } };
  state.datasetError = null;

  readBenchTail(benchmarksTail("datasets", "beta", null));
  assert.equal(state.benchDatasetName, "beta");
  assert.equal(state.dataset, null, "the stale dataset answer for alpha must not survive a switch to beta");

  // Re-reading the same tail (a reload, or a same-name hash write) must not
  // discard a still-valid cached answer.
  state.dataset = { dataset: { name: "beta", revision: 1, cases: [] } };
  readBenchTail(benchmarksTail("datasets", "beta", null));
  assert.notEqual(state.dataset, null, "re-reading the same dataset name must keep the cached answer");
});

test("readBenchTail drops a cached run answer whose id no longer matches the tail", () => {
  state.benchRunId = "run-1";
  state.benchRun = { run: { id: "run-1" }, results: [] };
  state.benchRunError = null;

  readBenchTail(benchmarksTail("runs", null, "run-2"));
  assert.equal(state.benchRunId, "run-2");
  assert.equal(state.benchRun, null, "the stale run answer for run-1 must not survive a switch to run-2");

  state.benchRun = { run: { id: "run-2" }, results: [] };
  readBenchTail(benchmarksTail("runs", null, "run-2"));
  assert.notEqual(state.benchRun, null, "re-reading the same run id must keep the cached answer");
});

test("a note id containing 'task' survives app.js's own task-modal split unharmed", () => {
  // The same faithful copy of `splitTail` `improvement.test.js` uses for
  // knowledge.js's `noteTail` -- proving the escape keeps the router's
  // MODAL cut from ever firing on a dataset name, not just the round trip.
  const MODAL = "task";
  function splitTail(tail) {
    const cut = tail.indexOf(MODAL);
    return cut < 0 ? [tail, []] : [tail.slice(0, cut), tail.slice(cut + 1)];
  }
  for (const name of ["task", "operations-task"]) {
    const tail = benchmarksTail("datasets", name, null);
    const [view, modal] = splitTail(tail);
    assert.deepEqual(view, tail);
    assert.deepEqual(modal, []);
  }
});

// ------------------------------------------------------------------ datasets

test("ungated is cases minus gated, never negative", () => {
  assert.equal(ungated({ cases: 5, gated: 2 }), 3);
  assert.equal(ungated({ cases: 0, gated: 0 }), 0);
  assert.equal(ungated({}), 0);
});

test("visibleCases narrows by scope without mutating the source array", () => {
  const cases = [{ id: "a", scope: "projects/quarry" }, { id: "b", scope: "projects/kiln" }];
  const kept = visibleCases(cases, (s) => s === "projects/quarry");
  assert.deepEqual(kept.map((c) => c.id), ["a"]);
  assert.equal(cases.length, 2, "the source array must be untouched");
});

test("importFormat reads jsonl/json/yaml/yml/csv and refuses everything else", () => {
  assert.equal(importFormat("cases.jsonl"), "jsonl");
  assert.equal(importFormat("CASES.JSON"), "json");
  assert.equal(importFormat("cases.yaml"), "yaml");
  assert.equal(importFormat("cases.yml"), "yml");
  assert.equal(importFormat("cases.csv"), "csv");
  assert.equal(importFormat("cases.txt"), null);
  assert.equal(importFormat("cases"), null);
  assert.equal(importFormat(""), null);
});

test("splitImportErrors turns the daemon's one joined message back into rows", () => {
  const message = "line 2, id: duplicate id \"a\"; line 3, scope: scope is required";
  assert.deepEqual(splitImportErrors(message), [
    'line 2, id: duplicate id "a"',
    "line 3, scope: scope is required",
  ]);
  assert.deepEqual(splitImportErrors(""), []);
  assert.deepEqual(splitImportErrors(null), []);
});

test("isSlug matches the daemon's own dataset-name and case-id pattern", () => {
  assert.ok(isSlug("registration"));
  assert.ok(isSlug("add-scope-2"));
  assert.ok(!isSlug("Bad Name"));
  assert.ok(!isSlug(""));
  assert.ok(!isSlug("-leading"));
});

test("agentChoices unions configurations' and the roster's agent names, deduplicated and sorted", () => {
  const configurations = [{ agents: [{ agent: "builder" }, { agent: "reviewer" }] }];
  const scopes = [{ agents: [{ name: "reviewer" }, { name: "foreman" }] }];
  assert.deepEqual(agentChoices(configurations, scopes), ["builder", "foreman", "reviewer"]);
  assert.deepEqual(agentChoices([], []), []);
});

test("filterTasksForPicker narrows by scope and status, both optional", () => {
  const tasks = [
    { id: "1", scope: "demo", status: "done" },
    { id: "2", scope: "demo", status: "failed" },
    { id: "3", scope: "other", status: "done" },
  ];
  assert.deepEqual(filterTasksForPicker(tasks, "demo", null).map((t) => t.id), ["1", "2"]);
  assert.deepEqual(filterTasksForPicker(tasks, null, "done").map((t) => t.id), ["1", "3"]);
  assert.deepEqual(filterTasksForPicker(tasks, "demo", "done").map((t) => t.id), ["1"]);
  assert.deepEqual(filterTasksForPicker(tasks, null, null).map((t) => t.id), ["1", "2", "3"]);
});

// ---------------------------------------------------------------- bench runs

test("progress counts settled attempts the same way the CLI's bench_run_line does", () => {
  const attempts = [{ verdict: "pass" }, { verdict: null }, { verdict: "fail" }];
  assert.deepEqual(progress(attempts), { settled: 2, total: 3 });
  assert.deepEqual(progress([]), { settled: 0, total: 0 });
  assert.deepEqual(progress(undefined), { settled: 0, total: 0 });
});

test("attemptState reads the verdict when there is one, else running, judging or pending", () => {
  assert.equal(attemptState({ verdict: "pass" }), "pass");
  assert.equal(attemptState({ verdict: "unverified" }), "unverified");
  assert.equal(attemptState({ verdict: null, task_id: "t1" }), "running");
  assert.equal(attemptState({ verdict: null, task_id: "t1" }, "running"), "running");
  assert.equal(attemptState({ verdict: null, task_id: null }), "pending");
});

test("attemptState reads judging off the task's own status, since BenchAttempt has no signal of its own for it", () => {
  // Judging happens on a background worker after the task's run has
  // settled (bench/engine.rs), so `verdict`/`run_id` arrive together, once,
  // only when the gate finishes -- the task's own terminal status is the
  // one thing that says "settled, gate not back yet" in the meantime.
  for (const status of ["done", "failed", "cancelled"]) {
    assert.equal(attemptState({ verdict: null, task_id: "t1" }, status), "judging");
  }
  for (const status of ["pending", "dispatching", "running", "blocked", undefined]) {
    assert.equal(attemptState({ verdict: null, task_id: "t1" }, status), "running");
  }
});

test("isGitRevish matches the daemon's own conservative base shape", () => {
  assert.ok(isGitRevish("3f9c2e1"));
  assert.ok(isGitRevish("origin/main"));
  assert.ok(isGitRevish("feature/x-1"));
  assert.ok(!isGitRevish("--detach"));
  assert.ok(!isGitRevish("a..b"));
  assert.ok(!isGitRevish(""));
  assert.ok(!isGitRevish(null));
});

test("matrixColumns mirrors the results order verbatim, so the table and the matrix never disagree", () => {
  const results = [
    { config_hash: "h1", label: "claude-code · opus" },
    { config_hash: "", label: "(no configuration -- the agent never resolved)" },
  ];
  assert.deepEqual(matrixColumns(results), [
    { config_hash: "h1", label: "claude-code · opus" },
    { config_hash: "", label: "(no configuration -- the agent never resolved)" },
  ]);
});

test("attemptsByCell groups by case id then config_hash, folding a missing config to the empty hash", () => {
  const attempts = [
    { case_id: "a", attempt: 1, config: { config_hash: "h1" } },
    { case_id: "a", attempt: 2, config: { config_hash: "h1" } },
    { case_id: "a", attempt: 1, config: { config_hash: "h2" } },
    { case_id: "b", attempt: 1, config: null },
  ];
  const cells = attemptsByCell(attempts);
  assert.equal(cells.a.h1.length, 2);
  assert.equal(cells.a.h2.length, 1);
  assert.equal(cells.b[""].length, 1);
});

test("scatterPoints only plots configurations with both a resolve rate and a mean wall-clock", () => {
  const results = [
    { config_hash: "h1", label: "a", resolve_rate: 0.5, mean_wall_clock_seconds: 120 },
    { config_hash: "h2", label: "b", resolve_rate: null, mean_wall_clock_seconds: 60 },
    { config_hash: "h3", label: "c", resolve_rate: 1, mean_wall_clock_seconds: null },
  ];
  const points = scatterPoints(results);
  assert.equal(points.length, 1);
  assert.deepEqual(points[0], { x: 120, y: 0.5, label: "a", config_hash: "h1" });
});
