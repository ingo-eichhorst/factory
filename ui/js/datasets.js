//! L5 Improvement, Benchmarks tab, Datasets segment: build a dataset by
//! hand, from recorded tasks, or by bulk import, and start a bench run
//! against it. The dataset list is company-wide (`state.datasets`); a
//! selected dataset's own case table narrows to the rail's scope selection
//! (`visibleCases`), following the v1 `visibleConfigurations` pattern.
//!
//! `gate` and `reset` run as the owner inside the attempt's own worktree --
//! the same trust the `shell` agent already carries -- and this view says so
//! plainly, in `datasets-note`, which is set from a constant rather than the
//! fetch so it survives a failed one.
//!
//! This file freely imports `modal.js` (for `scrim`) and `tasks.js` (to open
//! a case's origin task) -- both touch `document`, directly or through
//! `document.addEventListener`, so nothing here is imported by a Node test.
//! Every pure function a test needs instead lives in `bench-model.js`, which
//! this file also imports.

import { $, api, esc, state } from "./core.js";
import { writeHash } from "./scopes.js";
import { scrim, closeModal, dropModal } from "./modal.js";
import { openTask } from "./tasks.js";
import { setBenchSegment } from "./benchmarks.js";
import { loadBenchRuns, loadBenchRunDetail } from "./bench-runs.js";
import {
  agentChoices,
  filterTasksForPicker,
  importFormat,
  isGitRevish,
  isSlug,
  splitImportErrors,
  ungated,
  visibleCases,
} from "./bench-model.js";

const NOTE_TEXT =
  "The dataset list is company-wide -- the rail beside this page narrows only a selected dataset's own cases, " +
  "never which datasets are listed. gate and reset run as shell commands, as the owner, inside each attempt's " +
  "own git worktree -- the same trust the shell agent already carries.";

// ------------------------------------------------------------------ loading

/// Sets `state.datasets` (or `state.datasetsError`, never both), settles the
/// selection against the fresh list -- a deep link or a stale selection
/// naming a dataset that is gone is corrected rather than shown as an empty
/// detail panel -- and loads the selected dataset's own detail, if any.
export async function loadDatasets() {
  try {
    state.datasets = await api("/api/datasets");
    state.datasetsError = null;
  } catch (error) {
    state.datasets = null;
    state.datasetsError = error.message;
  }
  const list = state.datasetsError ? [] : (state.datasets && state.datasets.datasets) || [];
  if (state.benchDatasetName && !list.some((d) => d.name === state.benchDatasetName)) {
    state.benchDatasetName = null;
    state.dataset = null;
    state.datasetError = null;
    writeHash(true);
  }
  renderDatasetsSegment();
  if (state.benchDatasetName) await loadDataset(state.benchDatasetName);
}

export async function loadDataset(name) {
  try {
    state.dataset = await api(`/api/datasets/${encodeURIComponent(name)}`);
    state.datasetError = null;
  } catch (error) {
    state.dataset = null;
    state.datasetError = error.message;
  }
  renderDatasetsSegment();
}

function selectDataset(name) {
  if (name === state.benchDatasetName) return;
  state.benchDatasetName = name;
  state.dataset = null;
  state.datasetError = null;
  renderDatasetsSegment();
  writeHash();
  loadDataset(name);
}

// ------------------------------------------------------------------ render

function datasetRow(d) {
  const on = d.name === state.benchDatasetName ? ` aria-current="true"` : "";
  return `<tr class="row" data-name="${esc(d.name)}"${on}>
    <td><div class="title">${esc(d.name)}</div>${d.description ? `<div class="sub">${esc(d.description)}</div>` : ""}</td>
    <td>${esc(d.cases)}</td>
    <td>${esc(d.gated)}</td>
    <td>${esc(ungated(d))}</td>
    <td>${esc(d.revision)}</td>
  </tr>`;
}

function findingsBlock(findings) {
  if (!findings || !findings.length) return "";
  return `<div class="bench-findings">
    <h4>Findings</h4>
    <ul>${findings.map((f) => `<li><code>${esc(f.case)}</code> — ${esc(f.detail)}</li>`).join("")}</ul>
  </div>`;
}

function caseRow(c) {
  const origin = c.origin
    ? `<button type="button" class="linklike" data-open-task="${esc(c.origin.task)}">${esc(c.origin.task.slice(0, 8))}</button>
       <span class="sub">${esc(c.origin.outcome)}</span>`
    : `<span class="sub">—</span>`;
  return `<tr>
    <td><code>${esc(c.id)}</code></td>
    <td>${esc(c.title)}</td>
    <td>${esc(c.scope)}</td>
    <td>${esc(c.base || "scope HEAD")}</td>
    <td>${c.gate ? `<span title="${esc(c.gate)}">✓</span>` : `<span class="sub">unverified</span>`}</td>
    <td>${c.reset ? `<span title="${esc(c.reset)}">✓</span>` : `<span class="sub">—</span>`}</td>
    <td>${origin}</td>
    <td><button type="button" class="btn danger" data-del-case="${esc(c.id)}">Delete</button></td>
  </tr>`;
}

function renderDatasetDetail() {
  const el = $("dataset-detail");
  if (!el) return;
  if (!state.benchDatasetName) {
    el.hidden = true;
    return;
  }
  el.hidden = false;
  $("dataset-detail-name").textContent = state.benchDatasetName;

  // Guard against a stale answer even if one somehow survives the clear in
  // `readBenchTail` (an overlapping `loadDataset` from before the selection
  // changed, say) -- a cached dataset is only ever trusted for the name it
  // actually names.
  const data = state.datasetError || !state.dataset || state.dataset.dataset.name !== state.benchDatasetName
    ? null
    : state.dataset;
  if (!data) {
    $("dataset-detail-revision").textContent = state.datasetError || "loading…";
    $("dataset-description").textContent = "";
    $("dataset-description").hidden = true;
    $("dataset-findings").innerHTML = "";
    $("dataset-cases").innerHTML = "";
    $("no-cases").hidden = true;
    return;
  }
  const { dataset, findings } = data;
  $("dataset-detail-revision").textContent = `revision ${dataset.revision}`;
  // `.env-note` always carries a border and a background -- an empty one is
  // still a visible, empty box, so hide it outright rather than leaving it
  // showing nothing between the header and the case table.
  $("dataset-description").textContent = dataset.description || "";
  $("dataset-description").hidden = !dataset.description;
  $("dataset-findings").innerHTML = findingsBlock(findings);

  const cases = visibleCases(dataset.cases);
  $("dataset-cases").innerHTML = cases.map(caseRow).join("");
  $("no-cases").hidden = cases.length !== 0;
  for (const b of $("dataset-cases").querySelectorAll("[data-open-task]")) {
    b.onclick = () => openTask(b.dataset.openTask);
  }
  for (const b of $("dataset-cases").querySelectorAll("[data-del-case]")) {
    b.onclick = () => deleteCase(dataset.name, b.dataset.delCase);
  }
}

export function renderDatasetsSegment() {
  const note = $("datasets-note");
  if (note) note.textContent = NOTE_TEXT;
  const failed = $("datasets-error");
  if (failed) {
    failed.textContent = state.datasetsError || "";
    failed.hidden = !state.datasetsError;
  }

  const list = state.datasetsError ? [] : (state.datasets && state.datasets.datasets) || [];
  const rows = $("dataset-rows");
  if (rows) {
    rows.innerHTML = list.map(datasetRow).join("");
    for (const tr of rows.querySelectorAll("tr[data-name]")) {
      tr.onclick = () => selectDataset(tr.dataset.name);
    }
  }
  const empty = $("no-datasets");
  if (empty) empty.hidden = list.length !== 0 || !!state.datasetsError;

  renderDatasetDetail();
}

// -------------------------------------------------------------------- forms

function scopeOptions() {
  return (state.scopes || []).length
    ? state.scopes.map((s) => `<option value="${esc(s.name)}">${esc(s.name)}</option>`).join("")
    : `<option value="">(no scopes configured)</option>`;
}

function openNewDatasetForm() {
  dropModal();
  scrim(`
    <header><div><h2>New dataset</h2></div><button class="x" id="ds-close">&times;</button></header>
    <div class="body">
      <label for="ds-name">Name</label>
      <input id="ds-name" placeholder="registration">
      <label for="ds-description">Description <span class="sub" style="text-transform:none">optional</span></label>
      <textarea id="ds-description" rows="3"></textarea>
      <div class="err" id="ds-err"></div>
      <div class="row-btns" style="margin-top:16px">
        <button class="btn primary" id="ds-create">Create</button>
      </div>
    </div>`);
  $("ds-close").onclick = closeModal;
  $("ds-create").onclick = async () => {
    $("ds-err").textContent = "";
    const name = $("ds-name").value.trim();
    if (!isSlug(name)) {
      $("ds-err").textContent = `name ${JSON.stringify(name)} must match [a-z0-9][a-z0-9-]*`;
      return;
    }
    try {
      await api("/api/datasets", {
        method: "POST",
        body: JSON.stringify({ name, description: $("ds-description").value.trim() || null }),
      });
      closeModal();
      state.benchDatasetName = name;
      writeHash();
      await loadDatasets();
    } catch (e) {
      $("ds-err").textContent = e.message;
    }
  };
  $("ds-name").focus();
}

function openAddCaseForm(datasetName) {
  dropModal();
  scrim(`
    <header><div><h2>Add case</h2><code class="id">${esc(datasetName)}</code></div>
      <button class="x" id="ac-close">&times;</button></header>
    <div class="body">
      <div class="grid2">
        <div><label for="ac-id">Case id</label><input id="ac-id" placeholder="add-scope"></div>
        <div><label for="ac-scope">Scope</label><select id="ac-scope">${scopeOptions()}</select></div>
      </div>
      <label for="ac-title">Title</label>
      <input id="ac-title" placeholder="Register a new scope">
      <label for="ac-instructions">Instructions</label>
      <textarea id="ac-instructions" rows="6"></textarea>
      <div class="grid2">
        <div><label for="ac-base">Base commit <span class="sub" style="text-transform:none">optional -- absent means scope HEAD</span></label>
          <input id="ac-base"></div>
        <div><label for="ac-timeout">Timeout (seconds) <span class="sub" style="text-transform:none">optional</span></label>
          <input id="ac-timeout" type="number" min="1"></div>
      </div>
      <label for="ac-reset">Reset command <span class="sub" style="text-transform:none">optional -- runs in the attempt worktree before the agent starts</span></label>
      <input id="ac-reset" placeholder="./scripts/reset.sh">
      <label for="ac-gate">Gate command <span class="sub" style="text-transform:none">optional -- its exit status is the verdict; absent runs unverified</span></label>
      <input id="ac-gate" placeholder="./scripts/check.sh">
      <div class="err" id="ac-err"></div>
      <div class="row-btns" style="margin-top:16px">
        <button class="btn primary" id="ac-add">Add case</button>
      </div>
    </div>`);
  $("ac-close").onclick = closeModal;
  $("ac-add").onclick = async () => {
    $("ac-err").textContent = "";
    const id = $("ac-id").value.trim();
    if (!isSlug(id)) {
      $("ac-err").textContent = `id ${JSON.stringify(id)} must match [a-z0-9][a-z0-9-]*`;
      return;
    }
    const base = $("ac-base").value.trim() || null;
    if (base && !isGitRevish(base)) {
      $("ac-err").textContent =
        `base ${JSON.stringify(base)} is not a safe git revision (letters, digits, . _ / -, no leading -, no ..)`;
      return;
    }
    const c = {
      id,
      title: $("ac-title").value.trim(),
      scope: $("ac-scope").value,
      instructions: $("ac-instructions").value,
      base,
      reset: $("ac-reset").value.trim() || null,
      gate: $("ac-gate").value.trim() || null,
      timeout_seconds: $("ac-timeout").value.trim() ? parseInt($("ac-timeout").value, 10) : null,
    };
    try {
      await api(`/api/datasets/${encodeURIComponent(datasetName)}/cases`, {
        method: "POST",
        body: JSON.stringify({ cases: [c] }),
      });
      closeModal();
      await loadDataset(datasetName);
    } catch (e) {
      $("ac-err").textContent = e.message;
    }
  };
  $("ac-id").focus();
}

// No `failed`: a task whose run failed is `blocked` on it since #122.
const TASK_STATUSES = ["pending", "dispatching", "running", "blocked", "verifying", "done", "cancelled"];

function openFromTasksPicker(datasetName) {
  dropModal();
  const scopeNames = state.scopeNames || [];
  scrim(`
    <header><div><h2>From tasks</h2><code class="id">${esc(datasetName)}</code></div>
      <button class="x" id="ft-close">&times;</button></header>
    <div class="body">
      <div class="grid2">
        <div><label for="ft-scope">Scope</label>
          <select id="ft-scope"><option value="">(any)</option>${scopeNames.map((s) => `<option value="${esc(s)}">${esc(s)}</option>`).join("")}</select></div>
        <div><label for="ft-status">Status</label>
          <select id="ft-status"><option value="">(any)</option>${TASK_STATUSES.map((s) => `<option value="${s}">${s}</option>`).join("")}</select></div>
      </div>
      <label>Tasks <span class="sub" style="text-transform:none">recorded runs carry no configuration and no gate -- a generated case starts unverified</span></label>
      <div id="ft-list" class="picker-list"></div>
      <div class="err" id="ft-err"></div>
      <div class="row-btns" style="margin-top:16px">
        <button class="btn primary" id="ft-add">Add selected</button>
      </div>
    </div>`);
  $("ft-close").onclick = closeModal;

  function renderList() {
    const rows = filterTasksForPicker(
      [...state.tasks.values()],
      $("ft-scope").value || null,
      $("ft-status").value || null,
    ).sort((a, b) => b.created_at.localeCompare(a.created_at));
    $("ft-list").innerHTML = rows.length
      ? rows
          .map(
            (t) => `<label class="checkrow">
              <input type="checkbox" value="${esc(t.id)}">
              <span>${esc(t.title)} <span class="sub">${esc(t.scope)} · ${esc(t.status)}</span></span>
            </label>`,
          )
          .join("")
      : `<div class="sub">No tasks match.</div>`;
  }
  $("ft-scope").onchange = renderList;
  $("ft-status").onchange = renderList;
  renderList();

  $("ft-add").onclick = async () => {
    $("ft-err").textContent = "";
    const ids = [...$("ft-list").querySelectorAll("input:checked")].map((i) => i.value);
    if (!ids.length) {
      $("ft-err").textContent = "select at least one task";
      return;
    }
    try {
      await api(`/api/datasets/${encodeURIComponent(datasetName)}/from-tasks`, {
        method: "POST",
        body: JSON.stringify({ task_ids: ids }),
      });
      closeModal();
      await loadDataset(datasetName);
    } catch (e) {
      $("ft-err").textContent = e.message;
    }
  };
}

function openImportForm(datasetName) {
  dropModal();
  scrim(`
    <header><div><h2>Import</h2><code class="id">${esc(datasetName)}</code></div>
      <button class="x" id="im-close">&times;</button></header>
    <div class="body">
      <label for="im-file">File <span class="sub" style="text-transform:none">.jsonl, .json, .yaml, .yml or .csv</span></label>
      <input id="im-file" type="file" accept=".jsonl,.json,.yaml,.yml,.csv">
      <div class="checkrow"><input type="checkbox" id="im-replace"><span>Replace every existing case, instead of appending</span></div>
      <div class="err" id="im-err"></div>
      <div class="row-btns" style="margin-top:16px">
        <button class="btn primary" id="im-run">Import</button>
      </div>
    </div>`);
  $("im-close").onclick = closeModal;
  $("im-run").onclick = async () => {
    $("im-err").innerHTML = "";
    const file = $("im-file").files[0];
    if (!file) {
      $("im-err").textContent = "choose a file";
      return;
    }
    const format = importFormat(file.name);
    if (!format) {
      $("im-err").textContent = `unrecognised extension on "${file.name}" -- use .jsonl, .json, .yaml, .yml or .csv`;
      return;
    }
    try {
      const content = await file.text();
      await api(`/api/datasets/${encodeURIComponent(datasetName)}/import`, {
        method: "POST",
        body: JSON.stringify({ format, content, replace: $("im-replace").checked }),
      });
      closeModal();
      await loadDataset(datasetName);
    } catch (e) {
      // All-or-nothing: the daemon joins every problem into one message,
      // one line/row and field apiece -- split it back into rows rather
      // than showing the whole join as a single run-on sentence.
      $("im-err").innerHTML = splitImportErrors(e.message)
        .map((row) => `<div>${esc(row)}</div>`)
        .join("");
    }
  };
}

function openRunModal(datasetName, cases) {
  dropModal();
  const configurations = (state.benchmarks && state.benchmarks.configurations) || [];
  const agents = agentChoices(configurations, state.scopes);
  scrim(`
    <header><div><h2>Run…</h2><code class="id">${esc(datasetName)}</code></div>
      <button class="x" id="rm-close">&times;</button></header>
    <div class="body">
      <label>Agents</label>
      <div id="rm-agents" class="picker-list">
        ${agents.length ? agents.map((a) => `<label class="checkrow"><input type="checkbox" value="${esc(a)}"><span>${esc(a)}</span></label>`).join("") : `<div class="sub">No agents declared yet.</div>`}
      </div>
      <div class="grid2">
        <div><label for="rm-attempts">Attempts</label><input id="rm-attempts" type="number" min="1" max="10" value="1"></div>
        <div><label for="rm-concurrency">Concurrency</label><input id="rm-concurrency" type="number" min="1" value="1"></div>
      </div>
      <label>Cases <span class="sub" style="text-transform:none">none checked = every case</span></label>
      <div id="rm-cases" class="picker-list">
        ${cases.length ? cases.map((c) => `<label class="checkrow"><input type="checkbox" value="${esc(c.id)}"><span>${esc(c.id)} — ${esc(c.title)}</span></label>`).join("") : `<div class="sub">No cases yet.</div>`}
      </div>
      <div class="err" id="rm-err"></div>
      <div class="row-btns" style="margin-top:16px">
        <button class="btn primary" id="rm-start">Start run</button>
      </div>
    </div>`);
  $("rm-close").onclick = closeModal;
  $("rm-start").onclick = async () => {
    $("rm-err").textContent = "";
    const chosenAgents = [...$("rm-agents").querySelectorAll("input:checked")].map((i) => i.value);
    if (!chosenAgents.length) {
      $("rm-err").textContent = "choose at least one agent";
      return;
    }
    const attempts = Math.min(10, Math.max(1, parseInt($("rm-attempts").value, 10) || 1));
    const concurrency = Math.max(1, parseInt($("rm-concurrency").value, 10) || 1);
    const chosenCases = [...$("rm-cases").querySelectorAll("input:checked")].map((i) => i.value);
    try {
      const { run } = await api("/api/bench/runs", {
        method: "POST",
        body: JSON.stringify({
          dataset: datasetName,
          agents: chosenAgents,
          attempts,
          concurrency,
          cases: chosenCases.length ? chosenCases : null,
        }),
      });
      closeModal();
      state.benchRunId = run.id;
      setBenchSegment("runs");
      await loadBenchRuns();
      await loadBenchRunDetail(run.id);
    } catch (e) {
      $("rm-err").textContent = e.message;
    }
  };
}

// ------------------------------------------------------------------ delete

async function deleteCase(name, id) {
  if (!confirm(`Delete case "${id}" from "${name}"?`)) return;
  try {
    await api(`/api/datasets/${encodeURIComponent(name)}/cases/${encodeURIComponent(id)}`, { method: "DELETE" });
    await loadDataset(name);
  } catch (e) {
    alert(e.message);
  }
}

async function deleteDataset(name) {
  if (!name) return;
  if (!confirm(`Delete dataset "${name}"? This cannot be undone.`)) return;
  try {
    await api(`/api/datasets/${encodeURIComponent(name)}`, { method: "DELETE" });
    state.benchDatasetName = null;
    state.dataset = null;
    state.datasetError = null;
    writeHash();
    await loadDatasets();
  } catch (e) {
    alert(e.message);
  }
}

// -------------------------------------------------------------------- wire

export function wireDatasets() {
  $("dataset-new").onclick = () => openNewDatasetForm();
  $("dataset-add-case").onclick = () => {
    if (state.benchDatasetName) openAddCaseForm(state.benchDatasetName);
  };
  $("dataset-from-tasks").onclick = () => {
    if (state.benchDatasetName) openFromTasksPicker(state.benchDatasetName);
  };
  $("dataset-import").onclick = () => {
    if (state.benchDatasetName) openImportForm(state.benchDatasetName);
  };
  $("dataset-run").onclick = () => {
    if (!state.benchDatasetName || !state.dataset) return;
    openRunModal(state.benchDatasetName, visibleCases(state.dataset.dataset.cases));
  };
  $("dataset-delete").onclick = () => deleteDataset(state.benchDatasetName);
}
