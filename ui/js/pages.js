//! The Pages view (#343): the pages a scope's agents publish -- a list for the
//! selected scope and the scopes below it, and a page opened full width with
//! its version history and rollback. `pages-model.js` decides every shape and
//! attribute; this file fetches, draws and wires clicks.
//!
//! A page is never part of this document. It is framed from `/pages/...`
//! with `sandbox="allow-scripts"` and nothing else (`PAGE_SANDBOX`), and
//! given its data by `postMessage`. The conference-radar page also sends a
//! narrowly validated decision through this bridge; the parent saves it to
//! that scope's tracker. `dashboard.js` shows `page` tiles through the same
//! `wirePageFrames`, so a tile and this view hand a page its data the same way.
//!
//! A light poll while the view is on screen picks up a new version: a
//! publish is a run's report, and this file stays out of `app.js`'s event
//! wiring.

import { $, api, esc, state, currentTheme } from "./core.js";
import { writeHash } from "./scopes.js";
import { refLinks } from "./policy-model.js";
import {
  conferenceChoice,
  dataMessage,
  dataUrl,
  findPage,
  frameHtml,
  isReadyMessage,
  pageKey,
  pageRows,
  pagesTail,
  readPagesTail,
  shortSha,
  shownVersion,
  versionRows,
} from "./pages-model.js";

const POLL_MS = 30000;

let pages = null;
let failure = null;
let asked = 0;
let poll = null;
let wired = false;
/// `{scope, name, version}` -- the page opened, and a version picked to
/// preview (`null`: the current one).
let selection = null;
/// `sha -> {phase: "pending"|"error", message}` while a rollback is out.
const rollbacks = new Map();

// ------------------------------------------------------------------- frames

/// A version's data, fetched once per frame key and kept for the few frames
/// on screen -- the page itself never fetches it.
const dataCache = new Map();
const DATA_CACHE_SIZE = 8;
let listening = false;
let choiceWrites = Promise.resolve();
const choiceGeneration = new WeakMap();

function isRadarFrame(frame) {
  return frame?.dataset.pageScope === "conference-radar" && frame.dataset.pageName === "conference-radar";
}

function choiceUrl() {
  return "/api/pages/conference-choices?scope=conference-radar&name=conference-radar";
}

async function deliverChoices(frame) {
  if (!isRadarFrame(frame)) return;
  const generation = choiceGeneration.get(frame) || 0;
  try {
    const answer = await api(choiceUrl());
    if (frame.isConnected && (choiceGeneration.get(frame) || 0) === generation) {
      frame.contentWindow?.postMessage({ type: "conference-radar:decisions", choices: answer.choices }, "*");
    }
  } catch (error) {
    if (frame.isConnected) frame.contentWindow?.postMessage({ type: "conference-radar:error", message: error.message }, "*");
  }
}

async function saveChoice(frame, message) {
  try {
    const answer = await api("/api/pages/conference-choices", {
      method: "POST",
      body: JSON.stringify({
        scope: "conference-radar", name: "conference-radar",
        conference: message.conference, edition_year: message.edition_year, status: message.status,
      }),
    });
    if (frame.isConnected) frame.contentWindow?.postMessage({ type: "conference-radar:saved", choices: answer.choices }, "*");
  } catch (error) {
    if (frame.isConnected) frame.contentWindow?.postMessage({ type: "conference-radar:error", message: error.message }, "*");
  }
}

function cachedData(frame) {
  const key = frame.dataset.pageKey;
  if (frame.dataset.pageData !== "1") return Promise.resolve(null);
  if (!dataCache.has(key)) {
    const pending = api(dataUrl(frame.dataset.pageScope, frame.dataset.pageName, frame.dataset.pageVersion))
      .then((answer) => answer?.content?.data ?? null)
      .catch(() => {
        dataCache.delete(key);
        return null;
      });
    dataCache.set(key, pending);
    while (dataCache.size > DATA_CACHE_SIZE) dataCache.delete(dataCache.keys().next().value);
  }
  return dataCache.get(key);
}

async function deliver(frame) {
  const data = await cachedData(frame);
  const target = frame.contentWindow;
  if (!target) return;
  const message = dataMessage(
    { scope: frame.dataset.pageScope, name: frame.dataset.pageName, version: frame.dataset.pageVersion },
    data,
    currentTheme()
  );
  // `*`: the frame's origin is opaque by design, so there is no origin to
  // name. The frame is ours and `frame-src 'self'` keeps it on this host.
  target.postMessage(message, "*");
  void deliverChoices(frame);
}

function frameFor(source) {
  for (const frame of document.querySelectorAll("iframe[data-page-key]")) {
    if (frame.contentWindow === source) return frame;
  }
  return null;
}

function listen() {
  if (listening) return;
  listening = true;
  window.addEventListener("message", (event) => {
    const frame = frameFor(event.source);
    if (!frame) return;
    if (isReadyMessage(event.data)) { void deliver(frame); return; }
    if (!isRadarFrame(frame)) return;
    const msg = conferenceChoice(event.data);
    if (!msg) return;
    choiceGeneration.set(frame, (choiceGeneration.get(frame) || 0) + 1);
    choiceWrites = choiceWrites.catch(() => {}).then(() => saveChoice(frame, msg));
  });
  // A theme switch is news to every page on screen.
  document.addEventListener("factory:theme", () => {
    for (const frame of document.querySelectorAll("iframe[data-page-key]")) void deliver(frame);
  });
  // A second device can change a choice while this page stays open.
  setInterval(() => {
    for (const frame of document.querySelectorAll("iframe[data-page-key]")) {
      if (isRadarFrame(frame)) void deliverChoices(frame);
    }
  }, POLL_MS);
}

/// Hands every page frame under `root` its data once it loads. Called after
/// every draw that may have added one; a frame already wired is left alone.
export function wirePageFrames(root) {
  if (!root?.querySelectorAll || typeof window === "undefined") return;
  listen();
  for (const frame of root.querySelectorAll("iframe[data-page-key]")) {
    if (frame.dataset.wired) continue;
    frame.dataset.wired = "1";
    frame.addEventListener("load", () => void deliver(frame));
  }
}

// --------------------------------------------------------------------- view

export function showPages() {
  wire();
  void loadPages();
  stopPoll();
  poll = setInterval(() => void loadPages(), POLL_MS);
}

export function hidePages() {
  stopPoll();
}

function stopPoll() {
  if (poll) {
    clearInterval(poll);
    poll = null;
  }
}

export function pagesTailNow() {
  return pagesTail(selection);
}

export function readPagesTailNow(tail) {
  selection = readPagesTail(tail);
  if (pages) renderPages();
}

export async function loadPages() {
  const mine = ++asked;
  const query = state.scope !== null ? `?scope=${encodeURIComponent(state.scope)}` : "";
  try {
    const answer = await api(`/api/pages${query}`);
    if (mine !== asked) return;
    pages = answer.pages || [];
    failure = null;
  } catch (e) {
    if (mine !== asked) return;
    pages = null;
    failure = e.message;
  }
  renderPages();
}

function wire() {
  if (wired) return;
  wired = true;
  $("pages-refresh").onclick = () => void loadPages();
  $("pages-back").onclick = () => select(null);
}

function select(next) {
  selection = next;
  rollbacks.clear();
  renderPages();
  writeHash();
}

export function renderPages() {
  const error = $("pages-error");
  if (!error) return;
  error.hidden = !failure;
  error.textContent = failure ? `Could not load pages: ${failure}` : "";
  const page = selection && pages ? findPage(pages, selection.scope, selection.name) : null;
  $("pages-list-host").hidden = !!page;
  $("pages-detail").hidden = !page;
  $("pages-back").hidden = !page;
  if (page) renderDetail(page);
  else renderList();
}

function renderList() {
  const host = $("pages-list-host");
  const count = $("pages-count");
  if (pages === null) {
    host.innerHTML = failure ? "" : `<div class="empty">loading…</div>`;
    count.textContent = "";
    return;
  }
  const rows = pageRows(pages);
  count.textContent = `${rows.length} page${rows.length === 1 ? "" : "s"}`;
  if (!rows.length) {
    host.innerHTML = `<div class="empty">No page is declared here. A scope declares one under <code>scope.pages</code> in its
      <code>.factory/config.yaml</code>, and its agents publish it with <code>factory page publish</code>.</div>`;
    return;
  }
  host.innerHTML = `<table class="pages-table"><thead><tr>
      <th>Page</th><th>Scope</th><th>Version</th><th>Published</th><th>By</th><th>Kept</th>
    </tr></thead><tbody>${rows
      .map(
        (r) => `<tr class="pages-row" data-scope="${esc(r.scope)}" data-name="${esc(r.name)}" tabindex="0">
        <td><span class="pages-title">${esc(r.title)}</span><span class="sub"> ${esc(r.name)}</span></td>
        <td>${esc(r.scope)}</td>
        <td>${r.published ? `<code>${esc(r.version)}</code> <span class="sub">${esc(r.bytes)}</span>` : `<span class="sub">not published yet</span>`}</td>
        <td>${esc(r.when)}</td>
        <td>${esc(r.by)}</td>
        <td>${r.versions}</td>
      </tr>`
      )
      .join("")}</tbody></table>`;
  for (const row of host.querySelectorAll(".pages-row")) {
    const open = () => select({ scope: row.dataset.scope, name: row.dataset.name, version: null });
    row.onclick = open;
    row.onkeydown = (e) => {
      if (e.key === "Enter") open();
    };
  }
}

function runLink(scope, v) {
  if (!v?.taskId) return "";
  const links = refLinks(scope, [{ kind: "task", id: v.taskId }, ...(v.runId ? [{ kind: "run", id: v.runId }] : [])]);
  const link = links[links.length - 1];
  return link ? ` · <a href="${esc(link.href)}">${v.runId ? "run" : "task"}</a>` : "";
}

function renderDetail(page) {
  $("pages-count").textContent = `${page.scope} / ${page.name}`;
  const shown = shownVersion(page, selection.version);
  const previewing = shown && page.current && shown.sha256 !== page.current.sha256;
  const head = $("pages-detail-head");
  if (!shown) {
    head.innerHTML = `<h3>${esc(page.title)}</h3>
      <div class="empty">Not published yet. Declared by <code>${esc(page.scope)}</code>; its agents publish it with
      <code>factory page publish ${esc(page.name)} --file &lt;page.html&gt;</code>.</div>`;
  } else {
    const row = versionRows({ ...page, current: shown, versions: [shown] })[0];
    head.innerHTML = `<h3>${esc(shown.title || page.title)}</h3>
      <div class="sub">version <code>${esc(row.short)}</code> · ${esc(row.bytes)}${row.data ? ` · data ${esc(row.data)}` : ""}
        · ${esc(row.when)} · ${esc(row.by)}${runLink(page.scope, row)}</div>
      ${previewing ? `<div class="pages-preview">Previewing an older version -- the Pages list and the dashboard still show
        <code>${esc(shortSha(page.current.sha256))}</code>. <button class="btn small" id="pages-preview-end">Back to the current one</button></div>` : ""}`;
    const end = $("pages-preview-end");
    if (end) end.onclick = () => select({ ...selection, version: null });
  }
  renderVersions(page, shown);
  renderFrame(page, shown);
}

function renderVersions(page, shown) {
  const host = $("pages-versions");
  const rows = versionRows(page);
  $("pages-versions-count").textContent = `${rows.length} kept, at most ${page.keep}`;
  if (!rows.length) {
    host.innerHTML = "";
    return;
  }
  host.innerHTML = `<table class="pages-table"><thead><tr>
      <th>Version</th><th>Size</th><th>Published</th><th>By</th><th></th>
    </tr></thead><tbody>${rows
      .map((r) => {
        const pending = rollbacks.get(r.sha);
        const looking = shown && shown.sha256 === r.sha;
        const actions = r.current
          ? `<span class="badge s-done">shown</span>`
          : `${looking ? "" : `<button class="btn small" data-act="preview" data-sha="${esc(r.sha)}">Preview</button>`}
             <button class="btn small" data-act="rollback" data-sha="${esc(r.sha)}"${pending?.phase === "pending" ? " disabled" : ""}>Show this version</button>
             ${pending?.phase === "error" ? `<span class="err">${esc(pending.message)}</span>` : ""}`;
        return `<tr${looking ? ' class="on"' : ""}>
          <td><code>${esc(r.short)}</code></td>
          <td>${esc(r.bytes)}${r.data ? ` <span class="sub">data ${esc(r.data)}</span>` : ""}</td>
          <td>${esc(r.when)}</td>
          <td>${esc(r.by)}${runLink(page.scope, r)}</td>
          <td class="pages-actions">${actions}</td>
        </tr>`;
      })
      .join("")}</tbody></table>`;
  for (const b of host.querySelectorAll("button[data-act]")) {
    b.onclick = () => {
      if (b.dataset.act === "preview") select({ ...selection, version: b.dataset.sha });
      else void rollback(page, b.dataset.sha);
    };
  }
}

/// The frame is replaced only when what it should show changed: a poll that
/// finds the same version leaves the page, and whatever a person did in it,
/// alone.
function renderFrame(page, shown) {
  const host = $("pages-frame");
  if (!shown) {
    host.innerHTML = "";
    return;
  }
  const key = pageKey(page.scope, page.name, shown.sha256);
  const current = host.querySelector("iframe[data-page-key]");
  if (current && current.dataset.pageKey === key) return;
  host.innerHTML = frameHtml(page, shown, "page-frame page-frame-full");
  wirePageFrames(host);
}

async function rollback(page, sha) {
  rollbacks.set(sha, { phase: "pending" });
  renderPages();
  try {
    const answer = await api("/api/pages/rollback", {
      method: "POST",
      body: JSON.stringify({ scope: page.scope, name: page.name, version: sha }),
    });
    rollbacks.delete(sha);
    const index = pages.findIndex((p) => p.scope === page.scope && p.name === page.name);
    if (index >= 0 && answer.page) pages[index] = answer.page;
    select({ scope: page.scope, name: page.name, version: null });
  } catch (e) {
    rollbacks.set(sha, { phase: "error", message: e.message });
    renderPages();
  }
}
