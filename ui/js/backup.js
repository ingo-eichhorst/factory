//! L1 Backup (#116): is the company's state backed up, how recently, and
//! could it actually be restored? One answer from `/api/backup`, drawn as a
//! status hero, the honest warnings the daemon found, the history of
//! snapshots in the destination with a Verify on each, and what a snapshot
//! holds and leaves out.
//!
//! Two actions, both the owner's (the page speaks to the daemon as the
//! owner, like every page): **Back up now** and **Verify**. Each is
//! confirmed in a `modal.js` scrim -- never `alert`/`confirm` -- which then
//! stays up with the outcome, so a verification's every step is read where
//! it was asked for.
//!
//! Every formatter lives in `backup-model.js`, which the Node tests import;
//! this file only puts their answers on screen.

import { $, api, esc, state } from "./core.js";
import { closeModal, dropModal, scrim } from "./modal.js";
import {
  CONFIG_SNIPPET,
  MISSING,
  actions,
  ageBadge,
  backupFailure,
  checkRows,
  codeRows,
  destinationLevel,
  destinationText,
  fmtAgo,
  fmtBytes,
  fmtIn,
  fmtWhen,
  includeCount,
  keepText,
  keptByText,
  lastVerifiedText,
  nextDrillLevel,
  nextDrillText,
  rowVerifyState,
  scheduleText,
  timeMachineText,
  verifiedCell,
} from "./backup-model.js";

// ------------------------------------------------------------------ fetching

/// Sets `state.backup`, or `state.backupError` with `state.backupUnavailable`
/// saying whether the failure was only a daemon too old to serve the
/// endpoint. Renders nothing -- `refreshBackup` does both.
export async function loadBackup() {
  try {
    state.backup = (await api("/api/backup")).report;
    state.backupError = null;
    state.backupUnavailable = false;
  } catch (error) {
    state.backup = null;
    state.backupUnavailable = backupFailure(error) === "unavailable";
    state.backupError = state.backupUnavailable ? null : error.message;
  }
}

export async function refreshBackup() {
  const button = $("backup-refresh");
  if (button) button.disabled = true;
  try {
    await loadBackup();
    renderBackup();
  } finally {
    if (button) button.disabled = false;
  }
}

// ------------------------------------------------------------------ pieces

function fact(label, value, level) {
  const v = value === MISSING ? `<span class="infra-missing">${MISSING}</span>` : esc(value);
  return `<div class="infra-fact"><dt>${esc(label)}</dt><dd${level ? ` class="bk-${level}"` : ""}>${v}</dd></div>`;
}

function hero(report) {
  const badge = ageBadge(report);
  const newest = (report.snapshots || [])[0];
  const config = report.config;
  const headline = newest
    ? `<div class="bk-age">${esc(fmtAgo(report.now, newest.at))}</div>
       <div class="sub">newest backup · ${esc(fmtWhen(newest.at))} · ${esc(fmtBytes(newest.size_bytes))}</div>`
    : `<div class="bk-age">${config ? "No backup yet" : "Not configured"}</div>
       <div class="sub">${config ? "Nothing has been backed up to the destination." : "The instance's state is on this disk only."}</div>`;
  const dest = report.destination;
  const facts = config
    ? `<dl class="infra-facts">
        ${fact("Destination", dest ? dest.path : MISSING)}
        ${fact("Where it is", destinationText(dest), destinationLevel(dest))}
        ${fact("Schedule", scheduleText(config), config.schedule ? null : "warn")}
        ${fact("Next backup", report.next_run ? `${fmtIn(report.now, report.next_run)} · ${fmtWhen(report.next_run)}` : "only when somebody runs one")}
        ${fact("Next drill", nextDrillText(report), nextDrillLevel(report))}
        ${fact("Last verified", lastVerifiedText(report), report.last_verified ? (report.last_verified.ok ? "ok" : "bad") : "warn")}
        ${fact("Encrypted", config.encrypt_to
          ? `encrypted to ${config.encrypt_to}`
          : "no -- writes plaintext archives; keep the destination private")}
        ${fact("Retention", keepText(config))}
        ${fact("Logs", config.include_logs ? "included" : "not included")}
      </dl>`
    : `<p class="infra-hint">Factory backs up its own database, the configs and the authored content to a
        destination you name. Add a block like this to the root <code>.factory/config.yaml</code> and restart the
        daemon:</p>
      <pre class="infra-snippet"><code>${esc(CONFIG_SNIPPET)}</code></pre>`;
  return `<article class="infra-card bk-hero" data-level="${badge.level}" aria-labelledby="bk-hero-title">
    <header class="infra-card-head">
      <h3 id="bk-hero-title">Is the company's state backed up?</h3>
      <span class="tag bk-badge" data-level="${badge.level}">${esc(badge.label)}</span>
      ${report.running ? `<span class="tag warn">running</span>` : ""}
    </header>
    <div class="bk-headline">${headline}</div>
    ${facts}
  </article>`;
}

function warningStrip(warnings) {
  if (!warnings || !warnings.length) {
    return `<p class="env-note">No warnings: the newest backup is within its schedule, on another device, and has
      been verified.</p>`;
  }
  return `<ul class="bk-warnings" aria-label="Warnings">${warnings.map(w => `
    <li class="env-note ${w.level === "bad" ? "bad" : "warn"}" data-kind="${esc(w.kind)}">${esc(w.message)}</li>`).join("")}
  </ul>`;
}

/// `#155`: source code is backed up by pushing it, not by a snapshot, so
/// this reads `report.code`/`report.time_machine` regardless of whether
/// `report.config` is set -- unlike every other section on the page.
function codeSection(report) {
  const rows = codeRows(report);
  const tm = timeMachineText(report);
  const body = rows.length
    ? rows.map(r => `<tr>
        <td>${esc(r.scope)}</td>
        <td class="bk-path mono">${r.remote === MISSING ? `<span class="infra-missing">${MISSING}</span>` : esc(r.remote)}</td>
        <td class="bk-${r.level}">${esc(r.text)}</td>
      </tr>`).join("")
    : `<tr><td colspan="3" class="bk-none">no registered scope resolves to a repository</td></tr>`;
  return `<section class="bk-section"><h3>Code <span class="sub">pushed to a remote is the backup; this only reads what git already knows</span></h3>
    <div class="bk-scroll"><table><thead><tr><th>Scope</th><th>Remote</th><th>State</th></tr></thead><tbody>${body}</tbody></table></div>
    <dl class="infra-facts">${fact("Time Machine", tm.text, tm.level)}</dl>
  </section>`;
}

function history(report) {
  const rows = report.snapshots || [];
  if (!report.config) return "";
  if (!rows.length) {
    return `<section class="bk-section"><h3>History</h3><div class="empty">No snapshots in the destination yet.</div></section>`;
  }
  const can = actions(report);
  const body = rows.map(s => {
    const v = verifiedCell(s.verified);
    const verify = rowVerifyState(s);
    const disabled = !can.run || !verify.enabled;
    return `<tr>
      <td><div class="title bk-nowrap">${esc(fmtWhen(s.at))}</div><div class="sub mono bk-name">${esc(s.name)}
        ${s.encrypted ? `<span class="tag bk-enc" title="encrypted">encrypted</span>` : ""}</div></td>
      <td class="bk-nowrap">${esc(fmtBytes(s.size_bytes))}</td>
      <td>${typeof s.files === "number" ? s.files : `<span class="infra-missing" title="taken before this daemon's history began">${MISSING}</span>`}</td>
      <td class="bk-${v.level} bk-nowrap">${esc(v.text)}</td>
      <td>${esc(keptByText(s.kept_by))}</td>
      <td><button class="btn" data-verify="${esc(s.name)}" ${disabled ? "disabled" : ""} title="${esc(verify.title)}">Verify</button></td>
    </tr>`;
  }).join("");
  return `<section class="bk-section"><h3>History <span class="sub">newest first · ${rows.length} in the destination</span></h3>
    <div class="bk-scroll"><table>
      <thead><tr><th>Taken</th><th>Size</th><th>Files</th><th>Verified</th><th>Kept by</th><th></th></tr></thead>
      <tbody>${body}</tbody>
    </table></div></section>`;
}

function contents(report) {
  const include = (report.include || []).map(r => `<tr${r.included ? "" : ' class="bk-off"'}>
      <td class="bk-path"><code>${esc(r.path)}</code></td><td>${esc(r.why)}</td><td class="bk-nowrap">${esc(includeCount(r))}</td></tr>`).join("");
  const exclude = (report.exclude || []).map(r => `<tr><td class="bk-path"><code>${esc(r.path)}</code></td><td>${esc(r.why)}</td></tr>`).join("");
  return `<div class="bk-contents">
    <section class="bk-section"><h3>What's in it</h3>
      <div class="bk-scroll"><table><thead><tr><th>Path</th><th>Why</th><th>Newest snapshot</th></tr></thead><tbody>${include}</tbody></table></div>
    </section>
    <section class="bk-section"><h3>What's not</h3>
      <div class="bk-scroll"><table><thead><tr><th>Path</th><th>Why</th></tr></thead><tbody>${exclude}</tbody></table></div>
    </section>
  </div>`;
}

// ------------------------------------------------------------------ the view

export function renderBackup() {
  const unavailable = $("backup-unavailable");
  if (unavailable) unavailable.hidden = !state.backupUnavailable;
  const failed = $("backup-error");
  if (failed) {
    failed.textContent = state.backupError || "";
    failed.hidden = !state.backupError;
  }
  const report = state.backup;
  const can = actions(report);
  const run = $("backup-run");
  const verify = $("backup-verify");
  if (run) { run.disabled = !can.run; run.title = can.run ? "" : can.why; }
  if (verify) { verify.disabled = !can.verify; verify.title = can.verify ? "" : can.why; }

  const page = $("backup");
  if (!page) return;
  if (!report) {
    page.innerHTML = "";
    page.hidden = true;
    return;
  }
  page.hidden = false;
  page.innerHTML = [hero(report), warningStrip(report.warnings), codeSection(report), history(report), contents(report)].join("");
  for (const b of page.querySelectorAll("[data-verify]")) {
    b.onclick = () => confirmVerify(b.dataset.verify);
  }
}

// ------------------------------------------------------------------ actions

/// The one modal both actions use: a question, then -- once confirmed -- the
/// work in progress, then its outcome, all in the same scrim.
function ask({ title, id, body, go, label, work }) {
  dropModal();
  scrim(`<header><div><h2>${esc(title)}</h2>${id ? `<code class="id">${esc(id)}</code>` : ""}</div>
      <button class="x" id="bk-close">&times;</button></header>
    <div class="body" id="bk-body">${body}
      <div class="err" id="bk-err"></div>
      <div class="row-btns" style="margin-top:16px">
        <button class="btn primary" id="bk-go">${esc(label)}</button>
        <button class="btn" id="bk-cancel">Cancel</button>
      </div>
    </div>`);
  $("bk-close").onclick = closeModal;
  $("bk-cancel").onclick = closeModal;
  $("bk-go").onclick = async () => {
    const button = $("bk-go");
    button.disabled = true;
    button.textContent = work;
    $("bk-err").textContent = "";
    try {
      const outcome = await go();
      const body = $("bk-body");
      if (body) body.innerHTML = `${outcome}<div class="row-btns" style="margin-top:16px">
        <button class="btn" id="bk-done">Close</button></div>`;
      const done = $("bk-done");
      if (done) done.onclick = closeModal;
    } catch (e) {
      const err = $("bk-err");
      if (err) err.textContent = e.message;
      const again = $("bk-go");
      if (again) { again.disabled = false; again.textContent = label; }
    }
    // The events will also arrive over the socket; this redraws right away
    // for the person who just asked.
    refreshBackup();
  };
  $("bk-go").focus();
}

export function confirmRun() {
  const report = state.backup;
  const dest = report && report.destination ? report.destination.path : "the destination";
  ask({
    title: "Back up now",
    body: `<p class="infra-hint">Takes a snapshot of the database (a consistent copy while the daemon runs), the
      configs and the authored content into <code>${esc(dest)}</code>, then applies retention -- which may delete
      older snapshots no rule keeps. Secrets are never read or copied.</p>`,
    label: "Back up now",
    work: "Backing up…",
    go: async () => {
      const { snapshot } = await api("/api/backup/run", { method: "POST" });
      const pruned = snapshot.pruned && snapshot.pruned.length
        ? `Retention deleted ${snapshot.pruned.length}: ${snapshot.pruned.map(esc).join(", ")}.`
        : "Retention deleted nothing.";
      return `<p class="env-note">Backed up to <code>${esc(snapshot.path)}</code>: ${snapshot.files} files,
        ${esc(fmtBytes(snapshot.size_bytes))} archived (${esc(fmtBytes(snapshot.database_bytes))} database), in
        ${(snapshot.duration_ms / 1000).toFixed(1)}s. ${pruned}</p>
        <p class="infra-hint">Not yet proved to restore: verify it from the history.</p>`;
    },
  });
}

export function confirmVerify(name) {
  const report = state.backup;
  const target = name || (report && report.snapshots && report.snapshots[0] && report.snapshots[0].name);
  ask({
    title: "Verify",
    id: target,
    body: `<p class="infra-hint">Unpacks the snapshot into a temporary directory -- never over this instance --
      and proves it would restore: every checksum in its manifest, <code>integrity_check</code> on the database
      copy, and every authored-content loader. Nothing in the destination or the instance changes.</p>`,
    label: "Verify",
    work: "Verifying…",
    go: async () => {
      const query = name ? `?snapshot=${encodeURIComponent(name)}` : "";
      const { verification } = await api(`/api/backup/verify${query}`, { method: "POST" });
      const rows = checkRows(verification).map(c => `<tr>
          <td class="bk-${c.level}">${esc(c.status)}</td><td>${esc(c.name)}</td><td>${esc(c.detail)}</td></tr>`).join("");
      return `<p class="env-note ${verification.ok ? "" : "bad"}">${verification.ok
          ? "Verified: this snapshot would restore."
          : "Verification FAILED: this snapshot would not restore as it is."}
        <span class="sub">${(verification.duration_ms / 1000).toFixed(1)}s</span></p>
        <div class="bk-scroll"><table><thead><tr><th></th><th>Check</th><th>Detail</th></tr></thead><tbody>${rows}</tbody></table></div>`;
    },
  });
}

/// The bar's two buttons -- wired once, from app.js.
export function wireBackup() {
  const run = $("backup-run");
  if (run) run.onclick = () => confirmRun();
  const verify = $("backup-verify");
  if (verify) verify.onclick = () => confirmVerify(null);
  const refresh = $("backup-refresh");
  if (refresh) refresh.onclick = () => refreshBackup();
}
