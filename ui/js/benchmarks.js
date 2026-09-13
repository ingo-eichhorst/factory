//! L5 Improvement, tab 1: what Factory would need before a score means
//! anything. v1 declares and displays configurations -- it runs nothing and
//! shows no score, following the precedent of Secrets v1 (#49): the honest
//! inventory of what is missing, before the code that fills it in exists.
//! `loadBenchmarks` fetches `/api/benchmarks` into `state.benchmarks` (or
//! `state.benchmarksError`, never both) on every show, the one-answer shape
//! `sandboxes.js` uses for L2's fetch.

import { $, api, esc, state } from "./core.js";
import { inScope } from "./scopes.js";

const NOTE =
  "A score without its configuration is not a measurement -- the same model scores differently under a " +
  "different harness, tool surface or retry budget. Factory records no score yet. Every configuration below " +
  "is unpinned, which means recorded and excluded, never averaged.";

/// The seven-field tuple a result would have to carry to be comparable. The
/// labels are exactly the strings the daemon writes into a configuration's
/// `missing` array (`crates/factory-core/src/benchmark.rs`), so that array is
/// the single source of truth for which rows below read "not recorded" --
/// nothing here re-decides it.
const TUPLE_LABELS = ["harness", "harness version", "model", "tool surface", "context policy", "retry budget", "sandbox"];

/// The value a recorded field holds today. Three of the seven ever carry
/// one; the other four (harness version, tool surface, context policy, retry
/// budget) have no source at all yet, so they have nothing to show even when
/// `missing` somehow did not list them.
function fieldValue(config, label) {
  switch (label) {
    case "harness":
      return config.harness;
    case "model":
      return config.model_source === "args" ? config.model : null;
    case "sandbox":
      return config.sandbox;
    default:
      return null;
  }
}

function fieldRow(config, label) {
  const recorded = !(config.missing || []).includes(label);
  const value = fieldValue(config, label);
  let shown;
  if (recorded && value !== null && value !== undefined && value !== "") {
    shown = esc(value);
    if (label === "sandbox") shown += ` <span class="tag warn">declared, not enforced</span>`;
    if (label === "model") shown += ` <span class="tag">from args</span>`;
  } else {
    shown = `<span class="not-recorded">not recorded</span>`;
  }
  return `<div class="bench-field"><dt>${esc(label)}</dt><dd>${shown}</dd></div>`;
}

/// The checklist under every card: what the code lacks today before any of
/// these configurations could carry a score, drawn from the issue's "Later
/// increments" list (v2's dataset/gate machinery, v3's context hash). Fixed
/// here rather than in the payload -- it is a fact about the code, not about
/// any one configuration, so it stays on screen even when the fetch fails.
const NEEDS = [
  "A run per attempt -- only one run may be open per task today, so a case × configuration × attempt needs a task of its own.",
  "A retry loop -- a retry budget of N means N runs in a row, and nothing starts them yet.",
  "A pinned fixture base commit -- a worktree branches from whatever the scope's HEAD happens to be.",
  "Worktree cleanup -- worktrees are never removed, and attempts would pile up under .factory/worktrees/.",
  "A typed gate verdict -- exit status is free text today, not a schema.",
  "A configuration snapshot at dispatch -- a result has to record the whole tuple it ran with.",
  "A context hash -- the guide is regenerated on every call and never hashed (v3, after the above).",
];

/// Narrows configurations to the rail's inclusive scope selection, and each
/// kept configuration's own agent list the same way -- without mutating the
/// cached answer (`state.benchmarks`), so widening the selection again still
/// finds every agent a configuration ever declared.
export function visibleConfigurations(configs, contains = inScope) {
  return (configs || [])
    .map((c) => ({ ...c, agents: (c.agents || []).filter((a) => contains(a.scope)) }))
    .filter((c) => c.agents.length > 0);
}

function agentRow(a) {
  return `<li>${esc(a.scope)} / ${esc(a.agent)} <span class="sub">${esc(a.lifetime)}${a.declared === false ? " · synthesized" : ""}</span></li>`;
}

function configCard(config) {
  const badge = config.pinned
    ? `<span class="tag">pinned</span>`
    : `<span class="tag warn">unpinned — recorded, excluded</span>`;
  const modelLine =
    config.model_source === "args" && config.model
      ? `model <strong>${esc(config.model)}</strong> <span class="sub">(from args)</span>`
      : `model <span class="sub">harness default — not recorded</span>`;
  const flagsLine = (config.flags || []).length
    ? config.flags.map((f) => `<code>${esc(f)}</code>`).join(" ")
    : `<span class="sub">no flags recorded</span>`;
  return `<div class="bench-card">
    <div class="bench-card-head">
      <h3>${esc(config.harness)}</h3>
      ${badge}
    </div>
    <div class="bench-card-summary">
      <div>${modelLine}</div>
      <div class="bench-flags">${flagsLine}</div>
      <div>sandbox <strong>${esc(config.sandbox)}</strong> <span class="tag warn">declared, not enforced</span></div>
    </div>
    <details class="bench-details">
      <summary>Every field, recorded or not</summary>
      <dl class="bench-fields">${TUPLE_LABELS.map((l) => fieldRow(config, l)).join("")}</dl>
    </details>
    <div class="bench-agents">
      <h4>Declared by</h4>
      <ul>${(config.agents || []).map(agentRow).join("")}</ul>
    </div>
  </div>`;
}

export function renderBenchmarks() {
  const note = $("benchmarks-note");
  if (note) note.textContent = NOTE;

  const failed = $("benchmarks-error");
  if (failed) {
    failed.textContent = state.benchmarksError || "";
    failed.hidden = !state.benchmarksError;
  }

  const needs = $("benchmarks-needs");
  if (needs) needs.innerHTML = NEEDS.map((n) => `<li>${esc(n)}</li>`).join("");

  const all = state.benchmarksError ? [] : (state.benchmarks && state.benchmarks.configurations) || [];
  const configs = state.benchmarksError ? [] : visibleConfigurations(all);

  const count = $("benchmarks-count");
  if (count) {
    if (state.benchmarksError) {
      count.textContent = "";
    } else {
      const pinned = configs.filter((c) => c.pinned).length;
      count.textContent = `${configs.length} configuration${configs.length === 1 ? "" : "s"} · ${pinned} pinned`;
    }
  }

  const cards = $("benchmarks-cards");
  if (cards) cards.innerHTML = configs.map(configCard).join("");

  const empty = $("noBenchmarks");
  if (empty) empty.hidden = configs.length !== 0 || !!state.benchmarksError;
}

/// Sets `state.benchmarks` (or `state.benchmarksError`, never both). Called
/// on every show and from the Refresh button -- no polling, the same as L2:
/// the configuration list only changes when someone edits `config.yaml`.
export async function loadBenchmarks() {
  const btn = $("benchmarks-refresh");
  if (btn) btn.disabled = true;
  try {
    state.benchmarks = await api("/api/benchmarks");
    state.benchmarksError = null;
  } catch (error) {
    state.benchmarks = null;
    state.benchmarksError = error.message;
  } finally {
    if (btn) btn.disabled = false;
  }
  renderBenchmarks();
}
