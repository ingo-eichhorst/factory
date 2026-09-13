//! L5 Improvement, tab 2: a live view over the knowledge vault Factory keeps
//! at `<root>/.factory/knowledge/`. Nothing here writes a page, and no page's
//! text or document's bytes ever reach this file -- the browser gets titles,
//! tags, links and file metadata, the same promise `knowledge.rs` keeps on
//! the wire. `loadKnowledge` fetches `/api/knowledge` into `state.knowledge`
//! (or `state.knowledgeError`, never both) on every show, the same
//! one-answer-two-notes shape `sandboxes.js` uses for L2. "Add documents" is
//! the one write path this page has, and it only ever copies a file in.
//!
//! The graph's layout, its local-graph neighbourhood, its filter rules and
//! its hash-tail id encoding are all pure functions imported from
//! `knowledge-graph.js`, which never touches `document` -- see that file's
//! own doc comment, and `ui/tests/improvement.test.js`, which exercises them
//! with no DOM at all.

import { $, api, esc, state } from "./core.js";
import { writeHash } from "./scopes.js";
import {
  layoutGraph,
  nodeRadius,
  neighborhood,
  connectedNodeIds,
  isNodeDimmed,
  areaColorTokens,
  shouldShowAllLabels,
  unitsPerPixel,
  zoomAt,
  panBy,
  nodeTail,
  readNodeTail,
  encodeToolbarFlags,
  decodeToolbarFlags,
} from "./knowledge-graph.js";

// Assigned through `textContent` below, like `CORRECTION`/`BOUNDARY` in
// secrets.js -- never `esc()`, which is for `innerHTML` interpolation only
// and would double-escape a path containing `&`.
const INDEX_NOTE = (root) =>
  `Factory adds files to ${root || "this vault"} only when you add or import them, and never edits or deletes ` +
  "one. The browser sees titles, tags, links and file metadata, never a page's text or a document's contents.";

const SCOPE_NOTE =
  "The knowledge base is company-wide -- nothing on disk ties a page to a scope today, so the rail beside this " +
  "page does not filter it. Every page below is shown regardless of the current selection.";

// ------------------------------------------------------------------- state
//
// Module state, not `state.knowledge` itself: these are facts about what
// this view is showing right now, not about the last fetch -- the same
// reason `workflows.js` keeps `current`/`selectedNode` of its own.

/// The id of the page, `tag:<name>`, document path or gap target shown in
/// the inspector, or `null` for none.
let selected = null;
/// The node the pointer or keyboard focus is currently over, for the
/// neighbour-highlight -- separate from `selected`, which persists.
let hovered = null;
/// What the graph was last built from -- the payload it was drawn for, and
/// every piece of toolbar state that changes which nodes are eligible to
/// draw at all. Compared field by field in `needsRebuild` so a plain
/// selection change (which never removes or adds a node, outside local-graph
/// mode) only updates highlighting rather than tearing the SVG down and
/// stealing focus from a keyboard user mid-selection.
let graphBuiltFor = null;
/// The node list and edge list the graph was last actually drawn with --
/// kept so hovering or searching can look a node's label and neighbours up
/// without recomputing either from the payload on every pointer move.
let currentNodes = new Map();
let currentEdges = [];

const toolbar = { tags: true, documents: true, orphans: true, gaps: true, local: false, depth: 2 };
let search = "";
const view = { scale: 1, tx: 0, ty: 0 };
let dragging = null;
let wired = false;

// ----------------------------------------------------------------- hash tail
//
// The toolbar's state and the selection share the tail: `knowledgeTail`
// writes the toggles first (see `encodeToolbarFlags` in knowledge-graph.js),
// then the selection's own segments. A tail whose first segment does not
// match the flags shape (an old link, or none written yet) is read as no
// flags at all -- every toggle keeps its default, and the whole tail is the
// selection instead.

export function knowledgeTail() {
  return [encodeToolbarFlags(toolbar), ...nodeTail(selected)];
}

export function readKnowledgeTail(segments) {
  const [first, ...rest] = segments || [];
  const decoded = decodeToolbarFlags(first);
  if (decoded) {
    Object.assign(toolbar, decoded);
    selected = readNodeTail(rest);
  } else {
    selected = readNodeTail(segments);
  }
  renderKnowledge();
}

// ------------------------------------------------------------- graph building

function fileNameOf(path) {
  const i = String(path).lastIndexOf("/");
  return i < 0 ? path : path.slice(i + 1);
}

function areaOf(pageId) {
  const i = pageId.indexOf("/");
  return i < 0 ? "" : pageId.slice(0, i);
}

/// Every node the payload could draw, before the toolbar's toggles narrow
/// it -- a page for every page, and a tag/document/gap node only when its
/// toggle is on. The Orphans toggle is not applied here: it is decided by
/// `activeGraph` below, from the edges actually drawn, not from a node's
/// kind or its own fields (see `connectedNodeIds` in knowledge-graph.js).
function buildNodes(data) {
  const areaTokens = areaColorTokens((data.pages || []).map((p) => areaOf(p.id)));
  const nodes = [];
  for (const p of data.pages || []) {
    const area = areaOf(p.id);
    nodes.push({
      id: p.id,
      kind: "page",
      label: p.title,
      r: nodeRadius((p.backlinks || []).length),
      area,
      areaToken: areaTokens.get(area) || null,
    });
  }
  if (toolbar.tags) {
    for (const t of data.tags || []) nodes.push({ id: `tag:${t.name}`, kind: "tag", label: `#${t.name}`, r: 9 });
  }
  if (toolbar.documents) {
    for (const d of data.documents || []) {
      nodes.push({ id: d.id, kind: "document", label: fileNameOf(d.id), r: 9 });
    }
  }
  if (toolbar.gaps) {
    for (const g of data.gaps || []) nodes.push({ id: g.target, kind: "gap", label: g.target, r: 9 });
  }
  return nodes;
}

/// Every edge the payload names -- page to page, page to tag, page to
/// document, page to gap -- regardless of the toolbar. `layoutGraph` and
/// the renderer both only ever draw an edge whose two ends are both in the
/// node list they were given, so an edge into a toggled-off kind simply
/// never appears without this list needing to know about toggles at all.
function buildEdges(data) {
  const edges = [];
  for (const p of data.pages || []) {
    for (const t of p.links || []) edges.push([p.id, t, false]);
    for (const t of p.gaps || []) edges.push([p.id, t, true]);
    for (const t of p.tags || []) edges.push([p.id, `tag:${t}`, false]);
    for (const d of p.documents || []) edges.push([p.id, d, false]);
  }
  return edges;
}

/// The node and edge lists actually eligible to draw right now: every node
/// the kind toggles keep, then narrowed by Orphans (a node left with no
/// neighbour once the kind toggles already ran is what that toggle hides --
/// see `connectedNodeIds`), then further narrowed to the selected node's
/// neighbourhood when local-graph mode is on.
function activeGraph(data) {
  let nodes = buildNodes(data);
  const edges = buildEdges(data);
  if (!toolbar.orphans) {
    const connected = connectedNodeIds(
      nodes.map((n) => n.id),
      edges.map(([a, b]) => [a, b]),
    );
    nodes = nodes.filter((n) => connected.has(n.id));
  }
  if (!toolbar.local || !selected) return { nodes, edges };
  const scope = neighborhood(
    selected,
    edges.map(([a, b]) => [a, b]),
    toolbar.depth,
  );
  return { nodes: nodes.filter((n) => scope.has(n.id)), edges };
}

function needsRebuild(data) {
  const b = graphBuiltFor;
  if (!b || b.data !== data) return true;
  if (b.tags !== toolbar.tags || b.documents !== toolbar.documents) return true;
  if (b.orphans !== toolbar.orphans || b.gaps !== toolbar.gaps) return true;
  if (b.local !== toolbar.local || b.depth !== toolbar.depth) return true;
  if (toolbar.local && b.selected !== selected) return true;
  return false;
}

// -------------------------------------------------------------------- draw

function truncateLabel(s, max = 20) {
  const str = String(s ?? "");
  return str.length > max ? `${str.slice(0, max - 1)}…` : str;
}

/// A ring drawn behind every node's own shape, invisible until `.know-node`
/// carries `.selected` (see app.css) -- a fill/stroke recolour alone is not
/// reliably distinct on a tag/document/gap node (`.know-node.selected`'s
/// colours apply to any shape, but with dimming no longer marking everything
/// *else* around the selection, that recolour is the only cue left, and it
/// has to work regardless of the node's own kind or area colour).
function selectionRing(r) {
  return `<circle class="know-select-ring" r="${r + 5}"></circle>`;
}

function shapeFor(n) {
  const r = n.r;
  const ring = selectionRing(r);
  if (n.kind === "tag") return `${ring}<polygon points="0,${-r} ${r},0 0,${r} ${-r},0"></polygon>`;
  if (n.kind === "document") return `${ring}<rect x="${-r}" y="${-r}" width="${2 * r}" height="${2 * r}" rx="2"></rect>`;
  return `${ring}<circle r="${r}"></circle>`; // page and gap are both circles
}

function ariaLabelFor(n) {
  switch (n.kind) {
    case "tag":
      return `Tag: ${n.label}`;
    case "document":
      return `Document: ${n.label}`;
    case "gap":
      return `Gap: ${n.label}, linked but never written`;
    default:
      return `Page: ${n.label}`;
  }
}

function styleFor(n) {
  if (n.kind !== "page" || !n.areaToken) return "";
  return ` style="--node-fill: var(${n.areaToken}); --node-stroke: var(${n.areaToken});"`;
}

function buildGraphSvg(data) {
  const svg = $("knowledge-graph");
  if (!svg) return;
  const { nodes, edges } = activeGraph(data);
  const width = 640;
  const height = 420;
  const pos = layoutGraph(
    nodes.map((n) => ({ id: n.id, r: n.r })),
    edges.map(([a, b]) => [a, b]),
    { width, height },
  );
  currentNodes = new Map(nodes.map((n) => [n.id, n]));
  currentEdges = edges.filter(([a, b]) => pos[a] && pos[b]);

  const edgeSvg = currentEdges
    .map(([a, b, dashed]) => {
      const pa = pos[a];
      const pb = pos[b];
      return `<line class="know-edge${dashed ? " gap" : ""}" data-from="${esc(a)}" data-to="${esc(b)}"
        x1="${pa.x.toFixed(1)}" y1="${pa.y.toFixed(1)}" x2="${pb.x.toFixed(1)}" y2="${pb.y.toFixed(1)}"></line>`;
    })
    .join("");

  const ids = Object.keys(pos);
  const nodeSvg = ids
    .map((id) => {
      const n = currentNodes.get(id);
      const p = pos[id];
      return `<g class="know-node ${n.kind}" data-id="${esc(id)}" tabindex="0" role="button"
          aria-label="${esc(ariaLabelFor(n))}" transform="translate(${p.x.toFixed(1)},${p.y.toFixed(1)})"${styleFor(n)}>
        ${shapeFor(n)}
      </g>`;
    })
    .join("");

  // Labels are their own layer, drawn last so a dashed edge crossing a node
  // or a neighbouring node's circle can never paint over a label. Each also
  // carries a halo (`.know-label` in app.css) for the same reason where a
  // label still crosses an edge that runs behind it.
  const labelSvg = ids
    .map((id) => {
      const n = currentNodes.get(id);
      const p = pos[id];
      return `<text class="know-label" data-id="${esc(id)}"
          x="${p.x.toFixed(1)}" y="${(p.y + n.r + 12).toFixed(1)}" text-anchor="middle">${esc(truncateLabel(n.label))}</text>`;
    })
    .join("");

  svg.innerHTML =
    `<g class="know-view">` +
    `<g class="know-edges">${edgeSvg}</g><g class="know-nodes">${nodeSvg}</g><g class="know-labels">${labelSvg}</g>` +
    `</g>`;
  applyViewTransform();
  for (const g of svg.querySelectorAll(".know-node")) {
    g.onclick = () => selectNode(g.dataset.id);
    g.onkeydown = (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault(); // " " scrolls the page otherwise -- the node is not a link
        selectNode(g.dataset.id);
      }
    };
    g.onmouseenter = () => setHovered(g.dataset.id);
    g.onmouseleave = () => setHovered(null);
    g.onfocus = () => setHovered(g.dataset.id);
    g.onblur = () => setHovered(null);
  }
  updateGraphHighlight();
}

function setHovered(id) {
  if (hovered === id) return;
  hovered = id;
  updateGraphHighlight();
}

/// Selection, hover-neighbour highlighting, label visibility and the
/// search dim, all in one pass over the graph that is already on screen --
/// called after every rebuild, and on its own whenever only one of those
/// changed. Neighbour highlighting and dimming both follow hover/keyboard
/// focus alone, never the persistent selection -- see `isNodeDimmed` in
/// knowledge-graph.js for why. The selection is still marked (`.selected`,
/// styled in app.css to stand out on its own) and its edges/label lit, just
/// never by dimming everything else around it.
function updateGraphHighlight() {
  const svg = $("knowledge-graph");
  if (!svg) return;
  const focus = hovered;
  const neighbours = focus ? neighborhood(focus, currentEdges.map(([a, b]) => [a, b]), 1) : new Set();
  const query = search;
  const showAllLabels = shouldShowAllLabels(currentNodes.size, view.scale);

  for (const g of svg.querySelectorAll(".know-node")) {
    const id = g.dataset.id;
    const node = currentNodes.get(id);
    g.classList.toggle("selected", id === selected);
    g.classList.toggle("dim", !!node && isNodeDimmed(node, query, focus, neighbours));
  }
  for (const t of svg.querySelectorAll(".know-label")) {
    const id = t.dataset.id;
    const related = id === focus || neighbours.has(id);
    const show = id === selected || id === hovered || related || showAllLabels;
    t.classList.toggle("selected", id === selected);
    t.style.display = show ? "" : "none";
  }
  for (const line of svg.querySelectorAll(".know-edge")) {
    const touches = line.dataset.from === focus || line.dataset.to === focus;
    line.classList.toggle("selected", line.dataset.from === selected || line.dataset.to === selected);
    line.classList.toggle("dim", !!focus && !touches);
  }
}

/// The fixed viewBox `buildGraphSvg` draws into -- fixed regardless of the
/// SVG element's actual rendered size, which is what makes "the viewport
/// centre" a constant point in this space rather than something to look up
/// from the DOM on every zoom.
const VIEW_BOX = { width: 640, height: 420 };

function applyViewTransform() {
  const svg = $("knowledge-graph");
  const g = svg && svg.querySelector(".know-view");
  if (g) g.setAttribute("transform", `translate(${view.tx},${view.ty}) scale(${view.scale})`);
}

/// Zooms around `anchor` (viewBox-space; defaults to the viewport centre,
/// what the toolbar's +/- buttons use) so the content under it stays put --
/// see `zoomAt` in knowledge-graph.js for the math and why it replaced a
/// version that always zoomed around the origin instead.
function zoomBy(factor, anchor) {
  const next = zoomAt(view, factor, anchor || { x: VIEW_BOX.width / 2, y: VIEW_BOX.height / 2 });
  view.scale = next.scale;
  view.tx = next.tx;
  view.ty = next.ty;
  applyViewTransform();
  updateGraphHighlight(); // the label-by-zoom threshold may have just crossed
}

function zoomFit() {
  view.scale = 1;
  view.tx = 0;
  view.ty = 0;
  applyViewTransform();
  updateGraphHighlight();
}

function wireGraphInteraction() {
  const svg = $("knowledge-graph");
  if (!svg) return;
  svg.addEventListener(
    "wheel",
    (e) => {
      e.preventDefault();
      const rect = svg.getBoundingClientRect();
      const ppu = unitsPerPixel(VIEW_BOX, rect);
      const anchor = { x: (e.clientX - rect.left) * ppu, y: (e.clientY - rect.top) * ppu };
      zoomBy(e.deltaY < 0 ? 1.15 : 1 / 1.15, anchor);
    },
    { passive: false },
  );
  svg.addEventListener("mouseleave", () => setHovered(null));
  svg.addEventListener("mousedown", (e) => {
    if (e.target.closest(".know-node")) return; // dragging a node is not panning
    dragging = { x: e.clientX, y: e.clientY, tx: view.tx, ty: view.ty };
    svg.classList.add("panning");
  });
  window.addEventListener("mousemove", (e) => {
    if (!dragging) return;
    const rect = svg.getBoundingClientRect();
    if (!rect.width || !rect.height) return;
    const ppu = unitsPerPixel(VIEW_BOX, rect);
    const next = panBy({ tx: dragging.tx, ty: dragging.ty }, e.clientX - dragging.x, e.clientY - dragging.y, ppu);
    view.tx = next.tx;
    view.ty = next.ty;
    applyViewTransform();
  });
  window.addEventListener("mouseup", () => {
    if (dragging) {
      dragging = null;
      svg.classList.remove("panning");
    }
  });
}

// --------------------------------------------------------------- inspector

function goto(id) {
  return `<button type="button" class="linklike" data-goto="${esc(id)}">`;
}

function pageTitle(id, data) {
  const p = (data.pages || []).find((x) => x.id === id);
  return p ? p.title : id;
}

function pointsAtList(page, data) {
  const resolved = (page.links || []).map((id) => `<li>${goto(id)}${esc(pageTitle(id, data))}</button></li>`);
  const gaps = (page.gaps || []).map(
    (id) => `<li>${goto(id)}${esc(id)}</button> <span class="tag warn">unwritten</span></li>`,
  );
  return resolved.concat(gaps).join("") || `<li class="sub">Nothing.</li>`;
}

function pointedAtByList(ids, data) {
  const items = (ids || []).map((id) => `<li>${goto(id)}${esc(pageTitle(id, data))}</button></li>`).join("");
  return items || `<li class="sub">Nothing.</li>`;
}

function tagListFor(names) {
  const items = (names || []).map((n) => `<li>${goto(`tag:${n}`)}#${esc(n)}</button></li>`).join("");
  return items || `<li class="sub">None.</li>`;
}

function documentListFor(ids) {
  const items = (ids || []).map((id) => `<li>${goto(id)}${esc(fileNameOf(id))}</button></li>`).join("");
  return items || `<li class="sub">None.</li>`;
}

function formatBytes(n) {
  const v = Number(n) || 0;
  if (v < 1024) return `${v} B`;
  if (v < 1024 * 1024) return `${(v / 1024).toFixed(1)} KiB`;
  return `${(v / (1024 * 1024)).toFixed(1)} MiB`;
}

function renderPageInspector(page, data) {
  return `
    <h3>${esc(page.title)}</h3>
    <dl class="know-facts">
      <div><dt>Id</dt><dd><code>${esc(page.id)}</code></dd></div>
      <div><dt>Area</dt><dd>${esc(page.area || "—")}</dd></div>
      <div><dt>Status</dt><dd>${esc(page.status || "—")}</dd></div>
      <div><dt>Updated</dt><dd>${esc(page.updated || "—")}</dd></div>
      <div><dt>Sources</dt><dd>${esc(page.sources)}</dd></div>
    </dl>
    <h4>Tags</h4>
    <ul class="know-linklist">${tagListFor(page.tags)}</ul>
    <h4>Documents</h4>
    <ul class="know-linklist">${documentListFor(page.documents)}</ul>
    <h4>Points at</h4>
    <ul class="know-linklist">${pointsAtList(page, data)}</ul>
    <h4>Pointed at by</h4>
    <ul class="know-linklist">${pointedAtByList(page.backlinks, data)}</ul>`;
}

function renderTagInspector(tag, data) {
  return `
    <h3>#${esc(tag.name)}</h3>
    <h4>Pages</h4>
    <ul class="know-linklist">${pointedAtByList(tag.pages, data)}</ul>`;
}

function renderDocumentInspector(doc, data) {
  return `
    <h3>${esc(fileNameOf(doc.id))}</h3>
    <dl class="know-facts">
      <div><dt>Path</dt><dd><code>${esc(doc.id)}</code></dd></div>
      <div><dt>Type</dt><dd>${esc(doc.ext || "—")}</dd></div>
      <div><dt>Size</dt><dd>${esc(formatBytes(doc.bytes))}</dd></div>
    </dl>
    <h4>Referenced by</h4>
    <ul class="know-linklist">${pointedAtByList(doc.referenced_by, data)}</ul>`;
}

function renderGapInspector(target, data) {
  const gap = (data.gaps || []).find((g) => g.target === target);
  return `
    <h3><code>${esc(target)}</code></h3>
    <p class="sub">Linked but never written.</p>
    <h4>Pointed at by</h4>
    <ul class="know-linklist">${pointedAtByList(gap ? gap.from : [], data)}</ul>`;
}

function renderInspector(data) {
  const el = $("knowledge-inspector");
  if (!el) return;
  if (!selected) {
    el.innerHTML = `<p class="sub">Select a page, tag, document or gap in the graph, or a row in the tables below, to inspect it.</p>`;
    return;
  }
  const page = (data.pages || []).find((p) => p.id === selected);
  const tag = selected.startsWith("tag:") ? (data.tags || []).find((t) => `tag:${t.name}` === selected) : null;
  const doc = (data.documents || []).find((d) => d.id === selected);
  if (page) el.innerHTML = renderPageInspector(page, data);
  else if (tag) el.innerHTML = renderTagInspector(tag, data);
  else if (doc) el.innerHTML = renderDocumentInspector(doc, data);
  else el.innerHTML = renderGapInspector(selected, data); // whatever is left is a gap
  for (const b of el.querySelectorAll("[data-goto]")) b.onclick = () => selectNode(b.dataset.goto);
}

function selectNode(id) {
  if (id === selected) return;
  selected = id;
  renderKnowledge();
  writeHash();
}

// -------------------------------------------------------- gaps, findings, tables

function pageRow(p) {
  return `<tr>
    <td><div class="title">${goto(p.id)}${esc(p.title)}</button></div></td>
    <td><code>${esc(p.id)}</code></td>
    <td>${esc(p.area || "—")}</td>
    <td>${esc(p.status || "—")}</td>
    <td>${(p.backlinks || []).length}</td>
    <td>${(p.links || []).length}</td>
    <td>${(p.tags || []).length}</td>
    <td>${(p.documents || []).length}</td>
    <td>${esc(p.sources)}</td>
  </tr>`;
}

function tagRow(t) {
  return `<tr>
    <td>${goto(`tag:${t.name}`)}#${esc(t.name)}</button></td>
    <td>${(t.pages || []).length}</td>
  </tr>`;
}

function documentRow(d) {
  return `<tr>
    <td>${goto(d.id)}<code>${esc(d.id)}</code></button></td>
    <td>${esc(d.ext || "—")}</td>
    <td>${esc(formatBytes(d.bytes))}</td>
    <td>${(d.referenced_by || []).length}</td>
  </tr>`;
}

function gapRow(g) {
  return `<tr>
    <td>${goto(g.target)}<code>${esc(g.target)}</code></button></td>
    <td>${(g.from || []).length} — ${(g.from || []).map(esc).join(", ")}</td>
  </tr>`;
}

function findingsByKind(findings) {
  const groups = new Map();
  for (const f of findings || []) {
    if (!groups.has(f.kind)) groups.set(f.kind, []);
    groups.get(f.kind).push(f);
  }
  return groups;
}

/// The daemon's wire `kind` strings are `snake_case` identifiers meant for
/// code, not a heading -- shown raw they read as `SECRET_SOURCE`,
/// `MISSING_SOURCE`. This is the one place that translates each of the
/// seven kinds `knowledge.rs` can emit into the label the issue names for
/// it; a kind this map has never heard of (a future eighth finding) falls
/// back to the raw string rather than hiding it.
const FINDING_LABELS = {
  unsourced: "Unsourced pages",
  secret_source: "Sources under data/secrets/",
  missing_source: "Sources not found",
  incomplete_frontmatter: "Incomplete frontmatter",
  orphan: "Orphans — nothing connects here",
  ambiguous_link: "Ambiguous links",
  truncated: "Walk truncated",
};

export function findingLabel(kind) {
  return FINDING_LABELS[kind] || kind;
}

function renderFindings(findings) {
  const el = $("knowledge-findings");
  if (!el) return;
  const groups = findingsByKind(findings);
  el.innerHTML = [...groups.entries()]
    .map(
      ([kind, rows]) => `
    <div class="know-finding-group">
      <h4>${esc(findingLabel(kind))}</h4>
      <ul>${rows.map((r) => `<li><code>${esc(r.note)}</code> — ${esc(r.detail)}</li>`).join("")}</ul>
    </div>`,
    )
    .join("");
}

// ------------------------------------------------------------ add documents

/// Where a browser-uploaded file lands by default, the same rule
/// `factory knowledge add` uses on the CLI: the vault root for a `.md` file,
/// `documents/` for everything else.
function defaultUploadTarget(name) {
  return name.toLowerCase().endsWith(".md") ? name : `documents/${name}`;
}

function wireUpload() {
  const input = $("knowledge-upload");
  if (!input) return;
  input.onchange = async () => {
    const files = Array.from(input.files || []);
    input.value = ""; // the same file can be picked again later
    if (!files.length) return;
    const errors = [];
    for (const file of files) {
      const target = defaultUploadTarget(file.name);
      try {
        await api(`/api/knowledge/files?path=${encodeURIComponent(target)}&overwrite=false`, {
          method: "PUT",
          headers: {},
          body: file,
        });
      } catch (error) {
        errors.push(`${file.name}: ${error.message}`);
      }
    }
    const errEl = $("knowledge-upload-errors");
    if (errEl) {
      errEl.textContent = errors.join("\n");
      errEl.hidden = errors.length === 0;
    }
    loadKnowledge();
  };
}

// -------------------------------------------------------------------- wiring

/// Every control this view added beyond v1's Refresh button (which app.js
/// already wires at boot). Wired lazily, the first time this view actually
/// renders, so app.js never has to know these elements exist.
function wireToolbarOnce() {
  if (wired) return;
  wired = true;

  const searchEl = $("knowledge-search");
  if (searchEl) {
    searchEl.oninput = () => {
      search = searchEl.value.trim().toLowerCase();
      updateGraphHighlight();
    };
  }

  const bindToggle = (id, key) => {
    const el = $(id);
    if (!el) return;
    el.checked = toolbar[key];
    el.onchange = () => {
      toolbar[key] = el.checked;
      writeHash();
      renderKnowledge();
    };
  };
  bindToggle("knowledge-toggle-tags", "tags");
  bindToggle("knowledge-toggle-documents", "documents");
  bindToggle("knowledge-toggle-orphans", "orphans");
  bindToggle("knowledge-toggle-gaps", "gaps");
  bindToggle("knowledge-local", "local");

  const depthEl = $("knowledge-depth");
  if (depthEl) {
    depthEl.value = String(toolbar.depth);
    depthEl.onchange = () => {
      toolbar.depth = Number(depthEl.value) || 2;
      writeHash();
      renderKnowledge();
    };
  }

  const zoomIn = $("knowledge-zoom-in");
  if (zoomIn) zoomIn.onclick = () => zoomBy(1.25);
  const zoomOut = $("knowledge-zoom-out");
  if (zoomOut) zoomOut.onclick = () => zoomBy(1 / 1.25);
  const zoomFitBtn = $("knowledge-zoom-fit");
  if (zoomFitBtn) zoomFitBtn.onclick = () => zoomFit();

  wireGraphInteraction();
  wireUpload();
}

// ------------------------------------------------------------------- render

export function renderKnowledge() {
  wireToolbarOnce();

  const note = $("knowledge-note");
  if (note) note.textContent = INDEX_NOTE(state.knowledge && state.knowledge.root);
  const scopeNote = $("knowledge-scope-note");
  if (scopeNote) scopeNote.textContent = SCOPE_NOTE;

  const failed = $("knowledge-error");
  if (failed) {
    failed.textContent = state.knowledgeError || "";
    failed.hidden = !state.knowledgeError;
  }

  const data = state.knowledgeError ? null : state.knowledge;
  const present = !!(data && data.present);

  const empty = $("knowledge-empty");
  if (empty) {
    empty.hidden = !(data && data.present === false);
    if (data && data.present === false) {
      empty.textContent = data.legacy
        ? `No vault at ${data.root}. Found the old wiki at ${data.legacy} -- bring it in with: factory knowledge import ${data.legacy}`
        : `No vault at ${data.root}. Start one with: factory knowledge import <dir>`;
    }
  }

  const shell = $("knowledge-shell");
  if (shell) shell.hidden = !present;

  const count = $("knowledge-count");
  if (count) {
    if (present) {
      const pages = data.pages || [];
      const links = pages.reduce((n, p) => n + (p.links || []).length, 0);
      const gaps = (data.gaps || []).length;
      count.textContent = `${pages.length} page${pages.length === 1 ? "" : "s"} · ${links} link${links === 1 ? "" : "s"} · ${gaps} gap${gaps === 1 ? "" : "s"}`;
    } else {
      count.textContent = "";
    }
  }

  if (present) {
    if (needsRebuild(data)) {
      buildGraphSvg(data);
      graphBuiltFor = { data, ...toolbar, selected };
    } else {
      updateGraphHighlight();
    }
    renderInspector(data);
  } else {
    const svg = $("knowledge-graph");
    if (svg) svg.innerHTML = "";
    graphBuiltFor = null;
    currentNodes = new Map();
    currentEdges = [];
    const inspector = $("knowledge-inspector");
    if (inspector) inspector.innerHTML = "";
  }

  const pages = present ? data.pages || [] : [];
  const pagesBody = $("knowledge-pages");
  if (pagesBody) {
    pagesBody.innerHTML = pages.map(pageRow).join("");
    for (const b of pagesBody.querySelectorAll("[data-goto]")) b.onclick = () => selectNode(b.dataset.goto);
  }
  const noPages = $("knowledge-no-pages");
  if (noPages) noPages.hidden = !present || pages.length !== 0;

  const tags = present ? data.tags || [] : [];
  const tagsBody = $("knowledge-tags");
  if (tagsBody) {
    tagsBody.innerHTML = tags.map(tagRow).join("");
    for (const b of tagsBody.querySelectorAll("[data-goto]")) b.onclick = () => selectNode(b.dataset.goto);
  }
  const noTags = $("knowledge-no-tags");
  if (noTags) noTags.hidden = !present || tags.length !== 0;

  const documents = present ? data.documents || [] : [];
  const documentsBody = $("knowledge-documents");
  if (documentsBody) {
    documentsBody.innerHTML = documents.map(documentRow).join("");
    for (const b of documentsBody.querySelectorAll("[data-goto]")) b.onclick = () => selectNode(b.dataset.goto);
  }
  const noDocuments = $("knowledge-no-documents");
  if (noDocuments) noDocuments.hidden = !present || documents.length !== 0;

  const gaps = present ? data.gaps || [] : [];
  const gapsBody = $("knowledge-gaps");
  if (gapsBody) {
    gapsBody.innerHTML = gaps.map(gapRow).join("");
    for (const b of gapsBody.querySelectorAll("[data-goto]")) b.onclick = () => selectNode(b.dataset.goto);
  }
  const noGaps = $("knowledge-no-gaps");
  if (noGaps) noGaps.hidden = !present || gaps.length !== 0;

  renderFindings(present ? data.findings : []);
  const noFindings = $("knowledge-no-findings");
  if (noFindings) noFindings.hidden = !present || (data.findings || []).length !== 0;
}

/// The page with the most backlinks, or `null` when there are none to choose
/// from. A tie falls to whichever page the reduce visits first; since the
/// payload sorts pages by id, that is the alphabetically first of the tied
/// ids, which is at least a stable and repeatable answer.
function defaultPage(data) {
  const pages = data.pages || [];
  if (!pages.length) return null;
  return pages.reduce((best, p) => ((p.backlinks || []).length > (best.backlinks || []).length ? p : best)).id;
}

function knownId(data, id) {
  return (
    (data.pages || []).some((p) => p.id === id) ||
    (data.tags || []).some((t) => `tag:${t.name}` === id) ||
    (data.documents || []).some((d) => d.id === id) ||
    (data.gaps || []).some((g) => g.target === id)
  );
}

/// Called after every fetch: drops a selection the new answer does not know
/// about, fills in the default when there is none, and corrects the hash to
/// match -- a replace, never a push, the same "settle then correct" shape
/// `workflows.js`'s `readWorkflowTail` uses for the same reason.
function settleSelection() {
  const data = state.knowledgeError ? null : state.knowledge;
  if (!data || !data.present) return;
  if (selected && !knownId(data, selected)) selected = null;
  if (selected === null) selected = defaultPage(data);
  writeHash(true);
}

/// Sets `state.knowledge` (or `state.knowledgeError`, never both), settles
/// the selection against the fresh answer, and renders. Called on every show
/// and from the Refresh button -- no polling, the same as L2.
export async function loadKnowledge() {
  const btn = $("knowledge-refresh");
  if (btn) btn.disabled = true;
  try {
    state.knowledge = await api("/api/knowledge");
    state.knowledgeError = null;
  } catch (error) {
    state.knowledge = null;
    state.knowledgeError = error.message;
  } finally {
    if (btn) btn.disabled = false;
  }
  settleSelection();
  renderKnowledge();
}
