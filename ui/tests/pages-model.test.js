import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";

import {
  PAGE_SANDBOX,
  DATA_MESSAGE,
  READY_MESSAGE,
  pageKey,
  pageUrl,
  dataUrl,
  frameAttributes,
  frameHtml,
  dataMessage,
  isReadyMessage,
  conferenceChoice,
  formatBytes,
  pageRows,
  versionRows,
  shownVersion,
  pagesTail,
  readPagesTail,
  tilePage,
  tileFrameClass,
} from "../js/pages-model.js";
import { tileKind, validateTile, isTileRenderable } from "../js/dashboard-model.js";
import { buildCatalogue, entryToTile } from "../js/dashboard-editor-model.js";

const SHA_A = "a".repeat(64);
const SHA_B = "b".repeat(64);

function version(sha, at, extra) {
  return { sha256: sha, bytes: 2048, title: "Radar", published_at: at, published_by: "builder (worker) in radar", ...extra };
}

const PAGE = {
  scope: "ingo/radar",
  name: "conference-radar",
  title: "Konferenz-Radar",
  max_bytes: 20000000,
  keep: 10,
  current: version(SHA_B, "2026-10-19T08:00:00Z", { data_bytes: 512, run_id: "r2", task_id: "t1" }),
  versions: [
    version(SHA_B, "2026-10-19T08:00:00Z", { data_bytes: 512, run_id: "r2", task_id: "t1" }),
    version(SHA_A, "2026-10-12T08:00:00Z", { run_id: "r1", task_id: "t1" }),
  ],
};

// ----------------------------------------------------------------- isolation

test("a page frame is sandboxed with allow-scripts and nothing else -- never allow-same-origin", () => {
  assert.equal(PAGE_SANDBOX, "allow-scripts");
  const attrs = frameAttributes(PAGE, PAGE.current);
  assert.equal(attrs.sandbox, "allow-scripts");
  assert.equal(attrs.referrerpolicy, "no-referrer");
  const html = frameHtml(PAGE, PAGE.current);
  assert.match(html, /^<iframe [^>]*sandbox="allow-scripts"[^>]*><\/iframe>$/);
  assert.doesNotMatch(html, /allow-same-origin|allow-top-navigation|allow-popups|allow-forms/);
});

test("no UI source ever grants a frame the same origin", () => {
  const dir = new URL("../js/", import.meta.url);
  for (const file of readdirSync(dir)) {
    if (!file.endsWith(".js")) continue;
    const source = readFileSync(new URL(file, dir), "utf8");
    assert.doesNotMatch(source.replace(/\/\/.*$/gm, ""), /allow-same-origin/, `${file} names allow-same-origin outside a comment`);
  }
});

test("index.html runs no inline script, so the UI's CSP can say script-src 'self'", () => {
  const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
  const scripts = [...page.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/g)];
  assert.ok(scripts.length >= 2);
  for (const [, attrs, body] of scripts) {
    assert.match(attrs, /\bsrc="\/ui\/js\//, `an inline script: ${attrs}`);
    assert.equal(body.trim(), "", "a script element with a src has no body");
  }
  assert.match(page, /<script src="\/ui\/js\/theme-boot\.js"><\/script>/, "the theme still applies before the first paint");
  assert.doesNotMatch(page, / on[a-z]+="/, "no inline event handlers either");
  assert.match(page, /id="tab-pages"/);
  assert.match(page, /id="view-pages"/);
});

test("a page is served from /pages/ with each segment encoded, and its data from /api/pages/data", () => {
  assert.equal(pageUrl("radar", "conference-radar", SHA_A), `/pages/radar/conference-radar?v=${SHA_A}`);
  assert.equal(pageUrl("ingo/radar", "conference-radar"), "/pages/ingo/radar/conference-radar");
  assert.equal(pageUrl("a b", "p", ""), "/pages/a%20b/p");
  assert.equal(dataUrl("ingo/radar", "conference-radar", SHA_A), `/api/pages/data?scope=ingo%2Fradar&name=conference-radar&version=${SHA_A}`);
  assert.equal(pageKey("ingo/radar", "conference-radar", SHA_A), `ingo/radar/conference-radar@${SHA_A}`);
});

test("frame attributes carry what the data hand-over needs, escaped", () => {
  const attrs = frameAttributes(PAGE, PAGE.current, "page-frame page-frame-full");
  assert.equal(attrs.class, "page-frame page-frame-full");
  assert.equal(attrs["data-page-scope"], "ingo/radar");
  assert.equal(attrs["data-page-version"], SHA_B);
  assert.equal(attrs["data-page-data"], "1");
  assert.equal(frameAttributes(PAGE, PAGE.versions[1])["data-page-data"], "0");
  const hostile = { ...PAGE, title: `"><script>alert(1)</script>` };
  assert.doesNotMatch(frameHtml(hostile, PAGE.current), /<script>/);
});

test("the data message and ready handshake keep their own shapes", () => {
  const message = dataMessage({ scope: "radar", name: "conference-radar", version: SHA_A }, { n: 1 }, "foundry-light");
  assert.deepEqual(message, {
    type: DATA_MESSAGE,
    scope: "radar",
    name: "conference-radar",
    version: SHA_A,
    data: { n: 1 },
    theme: "foundry-light",
  });
  assert.equal(dataMessage({ scope: "s", name: "n" }).data, null);
  assert.equal(isReadyMessage({ type: READY_MESSAGE }), true);
  for (const other of [null, "factory:page-ready", { type: "factory:page-data" }, { type: "navigate", url: "/api/tasks" }]) {
    assert.equal(isReadyMessage(other), false, JSON.stringify(other));
  }
});

test("the conference choice bridge accepts only a bounded known decision", () => {
  const choice = { type: "conference-radar:choice", conference: "DemoConf", edition_year: "2027", status: "waitlisted" };
  assert.deepEqual(conferenceChoice(choice), { conference: "DemoConf", edition_year: "2027", status: "waitlisted" });
  for (const bad of [
    { ...choice, type: "navigate" },
    { ...choice, conference: "x".repeat(251) },
    { ...choice, edition_year: "../../etc" },
    { ...choice, status: "submitted<script>" },
  ]) assert.equal(conferenceChoice(bad), null);
});

// ------------------------------------------------------------------- shapes

test("the list says not published yet for a declared page with no version", () => {
  const rows = pageRows([PAGE, { scope: "radar", name: "roadmap", title: "Roadmap", versions: [] }]);
  assert.equal(rows[0].published, true);
  assert.equal(rows[0].version, SHA_B.slice(0, 12));
  assert.equal(rows[0].bytes, "2.0 kB");
  assert.equal(rows[0].when, "2026-10-19 08:00 UTC");
  assert.equal(rows[0].versions, 2);
  assert.equal(rows[1].published, false);
  assert.equal(rows[1].version, "");
});

test("version history marks the shown one, and a preview falls back when it is gone", () => {
  const rows = versionRows(PAGE);
  assert.deepEqual(rows.map((r) => [r.short, r.current]), [[SHA_B.slice(0, 12), true], [SHA_A.slice(0, 12), false]]);
  assert.equal(rows[0].data, "512 B");
  assert.equal(shownVersion(PAGE, SHA_A).sha256, SHA_A);
  assert.equal(shownVersion(PAGE, "c".repeat(64)).sha256, SHA_B, "a pruned preview shows the current one");
  assert.equal(shownVersion(PAGE, null).sha256, SHA_B);
  assert.equal(shownVersion({ ...PAGE, current: null, versions: [] }, null), null);
  assert.equal(formatBytes(1700000), "1.7 MB");
});

test("the view's URL tail round-trips a scope with a slash in it", () => {
  const selection = { scope: "ingo/radar", name: "conference-radar", version: SHA_A };
  assert.deepEqual(pagesTail(selection), ["ingo/radar", "conference-radar", SHA_A]);
  assert.deepEqual(readPagesTail(pagesTail(selection)), selection);
  assert.deepEqual(pagesTail({ scope: "radar", name: "p", version: null }), ["radar", "p"]);
  assert.equal(readPagesTail([]), null);
  assert.deepEqual(pagesTail(null), []);
});

// ---------------------------------------------------------- dashboard tiles

test("a page tile is a third tile kind, checked for shape only on this side", () => {
  const tile = { page: "conference-radar", size: "xl" };
  assert.equal(tileKind(tile), "page");
  assert.deepEqual(validateTile(tile), []);
  assert.deepEqual(validateTile({ page: "ingo/radar/conference-radar", size: "l" }), []);
  assert.ok(validateTile({ page: "Not A Page", size: "l" })[0].includes("unknown page tile"));
  assert.equal(tileKind({ page: "x", view: "kpis", size: "s" }), null, "one kind per tile");
  assert.equal(isTileRenderable(tile), true);
  assert.equal(tileFrameClass("xl"), "page-frame page-frame-tall");
  assert.equal(tileFrameClass("m"), "page-frame");
});

test("a tile resolves its page through the daemon's page_scopes, or its own scope prefix", () => {
  const pages = [PAGE];
  assert.equal(tilePage({ page: "conference-radar" }, { "conference-radar": "ingo/radar" }, pages).page, PAGE);
  assert.equal(tilePage({ page: "ingo/radar/conference-radar" }, {}, pages).page, PAGE);
  const missing = tilePage({ page: "elsewhere" }, {}, pages);
  assert.equal(missing.page, null);
  assert.equal(missing.name, "elsewhere");
  assert.equal(tilePage({ page: "conference-radar" }, { "conference-radar": "ingo/radar" }, undefined).page, null);
});

test("the Customise catalogue offers every declared page, bare in its own scope", () => {
  const catalogue = buildCatalogue([], [PAGE, { scope: "radar", name: "roadmap", title: "Roadmap" }], "ingo/radar");
  const pages = catalogue.filter((e) => e.kind === "page");
  assert.deepEqual(pages.map((e) => e.id), ["conference-radar", "radar/roadmap"]);
  assert.deepEqual(entryToTile(pages[1], "xl"), { page: "radar/roadmap", size: "xl" });
  assert.deepEqual(validateTile(entryToTile(pages[0], "xl")), []);
});
