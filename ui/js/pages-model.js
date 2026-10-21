//! Scope pages (#343), pure: where a page version is served, the one frame
//! every page is shown in, the message that hands a page its data, and the
//! rows the Pages view and a dashboard `page` tile draw. No DOM, no
//! `core.js`: `pages.js` and `dashboard.js` draw, this decides.
//!
//! A page is agent-authored HTML. It is never put into the UI's own
//! document: it is served from `/pages/...` under a strict CSP of its own and
//! shown in an `<iframe sandbox="allow-scripts">` -- scripts run, but in an
//! opaque origin, so the page cannot read this UI's storage or cookies, call
//! `/api/*` as the person looking at it, or reach the network at all. That
//! one attribute value is `PAGE_SANDBOX`, and nothing here ever adds
//! `allow-same-origin` to it.

/// The whole sandbox. `allow-scripts` alone: no same origin, no forms, no
/// popups, no top navigation, no downloads. The parent has one validated
/// conference-radar choice bridge; all other page messages are ignored.
export const PAGE_SANDBOX = "allow-scripts";

/// What the UI posts to a page: its version's `--data` (or `null`), and
/// which theme the UI is in. Posted once the frame has loaded, and again
/// whenever the page asks with `READY_MESSAGE`.
export const DATA_MESSAGE = "factory:page-data";
/// What a page may post to ask for its data.
export const READY_MESSAGE = "factory:page-ready";

/// The key a shown frame is kept by: the same scope, page and version is the
/// same frame, so a dashboard redraw can leave it alone instead of
/// reloading it.
export function pageKey(scope, name, sha) {
  return `${scope}/${name}@${sha || ""}`;
}

/// `/pages/<scope>/<name>?v=<sha256>` -- each segment encoded on its own, so
/// a scope name with a `/` in it stays a path, and the page is always the
/// last segment.
export function pageUrl(scope, name, sha) {
  const path = [...String(scope).split("/"), String(name)].map(encodeURIComponent).join("/");
  return sha ? `/pages/${path}?v=${encodeURIComponent(sha)}` : `/pages/${path}`;
}

/// `/api/pages/data?...` -- a version's data, fetched by the UI and posted
/// into the frame; the page never fetches anything itself.
export function dataUrl(scope, name, sha) {
  const params = new URLSearchParams({ scope, name });
  if (sha) params.set("version", sha);
  return `/api/pages/data?${params}`;
}

/// The frame's attributes, in the order they are written.
export function frameAttributes(page, version, cls) {
  const sha = version?.sha256 || "";
  return {
    class: cls || "page-frame",
    src: pageUrl(page.scope, page.name, sha),
    sandbox: PAGE_SANDBOX,
    referrerpolicy: "no-referrer",
    title: page.title || page.name,
    "data-page-key": pageKey(page.scope, page.name, sha),
    "data-page-scope": page.scope,
    "data-page-name": page.name,
    "data-page-version": sha,
    "data-page-data": version?.data_bytes != null ? "1" : "0",
  };
}

const escAttr = (s) =>
  String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

/// The frame as markup, for a view that draws with `innerHTML`.
export function frameHtml(page, version, cls) {
  const attrs = frameAttributes(page, version, cls);
  return `<iframe ${Object.entries(attrs)
    .map(([k, v]) => `${k}="${escAttr(v)}"`)
    .join(" ")}></iframe>`;
}

/// The message posted into a frame.
export function dataMessage(frame, data, theme) {
  return {
    type: DATA_MESSAGE,
    scope: frame.scope,
    name: frame.name,
    version: frame.version || null,
    data: data ?? null,
    theme: theme || null,
  };
}

/// Whether a message a frame posted asks for its data -- the only message
/// from a page the UI answers.
export function isReadyMessage(msg) {
  return !!msg && typeof msg === "object" && msg.type === READY_MESSAGE;
}

/// A choice from the conference-radar frame. The caller must also verify
/// that the sending frame is that declared page.
export function conferenceChoice(msg) {
  if (!msg || typeof msg !== "object" || msg.type !== "conference-radar:choice") return null;
  if (typeof msg.conference !== "string" || !msg.conference || msg.conference.length > 250) return null;
  if (typeof msg.edition_year !== "string" || !/^(20\d{2}|undated)$/.test(msg.edition_year)) return null;
  if (!["unreviewed", "not_interested", "not_this_time", "interested", "applied", "accepted", "waitlisted", "rejected", "withdrawn"].includes(msg.status)) return null;
  return { conference: msg.conference, edition_year: msg.edition_year, status: msg.status };
}

// ------------------------------------------------------------------- shapes

export function formatBytes(n) {
  const b = Number(n) || 0;
  if (b < 1000) return `${b} B`;
  if (b < 1000 * 1000) return `${(b / 1000).toFixed(b < 10000 ? 1 : 0)} kB`;
  return `${(b / 1000 / 1000).toFixed(1)} MB`;
}

export function shortSha(sha) {
  return String(sha || "").slice(0, 12);
}

/// `2026-10-19 14:05 UTC` -- the same spelling the CLI prints.
export function formatWhen(iso) {
  const t = Date.parse(iso);
  if (!Number.isFinite(t)) return "";
  const d = new Date(t);
  const pad = (n) => String(n).padStart(2, "0");
  return `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())} ${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())} UTC`;
}

/// One row per declared page for the Pages list: its title, where it is
/// declared, and its current version -- or "not published yet".
export function pageRows(pages) {
  return (pages || []).map((p) => {
    const v = p.current;
    return {
      key: `${p.scope}/${p.name}`,
      scope: p.scope,
      name: p.name,
      title: p.title || p.name,
      published: !!v,
      version: v ? shortSha(v.sha256) : "",
      bytes: v ? formatBytes(v.bytes) : "",
      when: v ? formatWhen(v.published_at) : "",
      by: v ? v.published_by : "",
      runId: v?.run_id || null,
      taskId: v?.task_id || null,
      versions: (p.versions || []).length,
    };
  });
}

/// A page's kept versions, newest first, each marked shown or not -- the
/// version history the Pages view offers rollback from.
export function versionRows(page) {
  const current = page?.current?.sha256;
  return (page?.versions || []).map((v) => ({
    sha: v.sha256,
    short: shortSha(v.sha256),
    bytes: formatBytes(v.bytes),
    data: v.data_bytes != null ? formatBytes(v.data_bytes) : "",
    when: formatWhen(v.published_at),
    by: v.published_by,
    runId: v.run_id || null,
    taskId: v.task_id || null,
    current: v.sha256 === current,
  }));
}

/// The page a scope/name pair names in a list, or null.
export function findPage(pages, scope, name) {
  return (pages || []).find((p) => p.scope === scope && p.name === name) || null;
}

/// The version a person is looking at: the one they picked to preview, if
/// it is still kept, else the current one.
export function shownVersion(page, previewSha) {
  if (!page) return null;
  if (previewSha) {
    const found = (page.versions || []).find((v) => v.sha256 === previewSha);
    if (found) return found;
  }
  return page.current || null;
}

/// The Pages view's own URL segments: `[scope, name]`, plus a previewed
/// version. A scope name may contain `/`; the router encodes each segment.
export function pagesTail(selection) {
  if (!selection || !selection.scope || !selection.name) return [];
  return selection.version ? [selection.scope, selection.name, selection.version] : [selection.scope, selection.name];
}

export function readPagesTail(tail) {
  const [scope, name, version] = tail || [];
  return scope && name ? { scope, name, version: version || null } : null;
}

// --------------------------------------------------------- dashboard tiles

/// The page a dashboard `page` tile shows: `pageScopes` is the dashboard
/// answer's own `page_scopes` (the daemon resolves which scope declares a
/// bare name), and the page name is the reference's last segment.
export function tilePage(tile, pageScopes, pages) {
  const ref = typeof tile?.page === "string" ? tile.page : "";
  const name = ref.split("/").pop();
  const scope = (pageScopes || {})[ref] || (ref.includes("/") ? ref.slice(0, ref.lastIndexOf("/")) : null);
  if (!scope || !name) return { scope, name, page: null };
  return { scope, name, page: findPage(pages, scope, name) };
}

/// The frame class a tile of `size` gets -- taller the wider it is.
export function tileFrameClass(size) {
  return size === "xl" || size === "l" ? "page-frame page-frame-tall" : "page-frame";
}
