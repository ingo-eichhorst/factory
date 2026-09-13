//! L5 Improvement, tab 2: a read-only index over the wiki on disk. v1 indexes;
//! it writes nothing and never ships a page's body -- the browser gets
//! titles, frontmatter fields, links and findings, and a reader who wants the
//! text opens the file. `loadKnowledge` fetches `/api/knowledge` into
//! `state.knowledge` (or `state.knowledgeError`, never both) on every show,
//! the same one-answer-two-notes shape `sandboxes.js` uses for L2, except
//! there is only one tab here to feed.
//!
//! The graph's layout is a pure function (`layoutGraph`) so the same wiki
//! draws the same picture on every load -- no `Math.random`, a seed derived
//! from the sorted node ids, a fixed iteration count. Selecting a node writes
//! the id to the hash tail (`knowledgeTail`/`readKnowledgeTail`) so a link
//! survives reload and back/forward; see the comment on `noteTail` for why a
//! note id's segments cannot be written to the tail verbatim.

import { $, api, esc, state } from "./core.js";
import { writeHash } from "./scopes.js";

// Assigned through `textContent` below, like `CORRECTION`/`BOUNDARY` in
// secrets.js -- never `esc()`, which is for `innerHTML` interpolation only
// and would double-escape a path containing `&`.
const INDEX_NOTE = (root) =>
  `The index below is derived from the Markdown files under ${root || "the wiki root"} on every ` +
  "request -- there is no link table kept in step with the disk, and nothing on this page writes a note. A " +
  "reader who wants a page's text opens the file; the browser only ever sees titles, frontmatter and links.";

const SCOPE_NOTE =
  "The knowledge base is company-wide -- nothing on disk ties a page to a scope today, so the rail beside this " +
  "page does not filter it. Every note below is shown regardless of the current selection.";

// ------------------------------------------------------------ deterministic layout
//
// A tiny seeded PRNG (mulberry32) rather than `Math.random`, seeded from a
// hash of the node ids themselves -- never from insertion order, never from
// wall-clock time -- so the same wiki produces the same picture on every
// load, in every browser, regardless of what order the payload happened to
// list notes and gaps in.

function hashSeed(ids) {
  let h = 2166136261; // FNV-1a offset basis
  for (const id of ids) {
    for (let i = 0; i < id.length; i++) {
      h ^= id.charCodeAt(i);
      h = Math.imul(h, 16777619);
    }
    h ^= 47; // a separator, so ["ab","c"] and ["a","bc"] do not collide
  }
  return h >>> 0;
}

function mulberry32(seed) {
  let a = seed >>> 0;
  return function () {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/// A node's on-screen radius: bigger for a note more other notes point at, so
/// the graph's own geometry says which pages the wiki leans on without
/// reading the table beside it.
const BASE_RADIUS = 10;
const RADIUS_PER_BACKLINK = 2;
const MAX_RADIUS = 26;
export function nodeRadius(backlinks) {
  const n = Number(backlinks) || 0;
  return Math.min(MAX_RADIUS, BASE_RADIUS + RADIUS_PER_BACKLINK * n);
}

/// `notes` and `gaps` are the payload's own arrays; `{width, height}` is the
/// viewBox the caller intends to draw into. Returns `{id: {x, y}}` for every
/// note id and every gap target, pure and deterministic: the same two arrays,
/// in any order, produce the same coordinates every time, and every
/// coordinate lands within the box. A force layout -- pairwise repulsion, a
/// spring per resolved or gap edge, a pull to centre, a fixed number of
/// iterations rather than a convergence check (a convergence check is itself
/// a source of nondeterminism across engines) -- then a final clamp so no
/// node's circle can draw outside the frame.
export function layoutGraph(notes, gaps, { width = 640, height = 420 } = {}) {
  const noteList = notes || [];
  const gapList = gaps || [];
  const backlinksOf = new Map(noteList.map((n) => [n.id, (n.backlinks || []).length]));
  // Sorted explicitly, and not trusted from the caller's order -- seeding and
  // iterating both depend on it, and a fixture built in a different order
  // must still land on the same picture.
  const ids = Array.from(new Set([...noteList.map((n) => n.id), ...gapList.map((g) => g.target)])).sort();
  if (!ids.length) return {};

  const rand = mulberry32(hashSeed(ids));
  const pos = {};
  for (const id of ids) {
    pos[id] = {
      x: width / 2 + (rand() - 0.5) * width * 0.7,
      y: height / 2 + (rand() - 0.5) * height * 0.7,
    };
  }

  const edges = [];
  for (const n of noteList) {
    for (const t of n.links || []) if (pos[t]) edges.push([n.id, t]);
    for (const t of n.gaps || []) if (pos[t]) edges.push([n.id, t]);
  }

  const cx = width / 2;
  const cy = height / 2;
  const ITERATIONS = 240;
  for (let iter = 0; iter < ITERATIONS; iter++) {
    const disp = new Map(ids.map((id) => [id, { x: 0, y: 0 }]));

    // Repulsion, every pair -- keeps unrelated notes from stacking.
    for (let i = 0; i < ids.length; i++) {
      for (let j = i + 1; j < ids.length; j++) {
        const a = ids[i];
        const b = ids[j];
        let dx = pos[a].x - pos[b].x;
        let dy = pos[a].y - pos[b].y;
        let d2 = dx * dx + dy * dy;
        if (d2 < 0.02) {
          dx = 0.15;
          dy = 0.1;
          d2 = 0.03;
        }
        const d = Math.sqrt(d2);
        const force = 2400 / d2;
        const fx = (dx / d) * force;
        const fy = (dy / d) * force;
        disp.get(a).x += fx;
        disp.get(a).y += fy;
        disp.get(b).x -= fx;
        disp.get(b).y -= fy;
      }
    }

    // Springs, one per edge -- pulls a linked note and its target together.
    for (const [a, b] of edges) {
      const dx = pos[b].x - pos[a].x;
      const dy = pos[b].y - pos[a].y;
      const d = Math.max(0.1, Math.sqrt(dx * dx + dy * dy));
      const force = (d - 92) * 0.018;
      const fx = (dx / d) * force;
      const fy = (dy / d) * force;
      disp.get(a).x += fx;
      disp.get(a).y += fy;
      disp.get(b).x -= fx;
      disp.get(b).y -= fy;
    }

    // A gentle pull to centre, so a graph with few edges does not drift.
    for (const id of ids) {
      const d = disp.get(id);
      d.x += (cx - pos[id].x) * 0.012;
      d.y += (cy - pos[id].y) * 0.012;
    }

    const CAP = 14;
    for (const id of ids) {
      const d = disp.get(id);
      const len = Math.sqrt(d.x * d.x + d.y * d.y) || 1;
      const capped = Math.min(len, CAP);
      pos[id].x += (d.x / len) * capped;
      pos[id].y += (d.y / len) * capped;
    }
  }

  // The force iterations settle into whatever cluster the springs and
  // repulsion happen to agree on, which is usually much smaller than the
  // viewBox -- left as-is, a small wiki draws as a tight knot in the middle
  // of a mostly empty box. Scale-and-translate the settled bounding box up
  // to fill the viewBox instead, leaving room on every side for a label
  // (~60px so a wide title never clips at the edge, less on top/bottom where
  // labels sit only below a node). This runs before the per-node radius
  // clamp below, so whatever this step produces still gets pulled back
  // inside the frame -- scaling first and clamping after, never the other
  // order, or a scaled-up node could land outside the viewBox the clamp is
  // meant to guarantee.
  const PAD_X = 60;
  const PAD_TOP = 24;
  const PAD_BOTTOM = 36;
  const MAX_SCALE = 2.5;
  let minX = Infinity;
  let maxX = -Infinity;
  let minY = Infinity;
  let maxY = -Infinity;
  for (const id of ids) {
    minX = Math.min(minX, pos[id].x);
    maxX = Math.max(maxX, pos[id].x);
    minY = Math.min(minY, pos[id].y);
    maxY = Math.max(maxY, pos[id].y);
  }
  const spreadX = maxX - minX;
  const spreadY = maxY - minY;
  const usableW = Math.max(1, width - 2 * PAD_X);
  const usableH = Math.max(1, height - PAD_TOP - PAD_BOTTOM);
  // Each axis is guarded on its own: two nodes stacked exactly vertically
  // give `spreadX === 0` without the graph being a single point, and scaling
  // by the other axis alone (rather than falling back to no scale at all)
  // is what actually fills the box in that case.
  let scale = 1;
  if (spreadX > 0 && spreadY > 0) {
    scale = Math.min(usableW / spreadX, usableH / spreadY);
  } else if (spreadX > 0) {
    scale = usableW / spreadX;
  } else if (spreadY > 0) {
    scale = usableH / spreadY;
  } // else every node coincides (typically just one node) -- no scale, only centring below.
  scale = Math.min(MAX_SCALE, scale);
  const midX = (minX + maxX) / 2;
  const midY = (minY + maxY) / 2;
  const targetCx = width / 2;
  const targetCy = PAD_TOP + usableH / 2;
  for (const id of ids) {
    pos[id].x = targetCx + (pos[id].x - midX) * scale;
    pos[id].y = targetCy + (pos[id].y - midY) * scale;
  }

  // The bounds every acceptance test checks: no circle may draw outside the
  // viewBox, clamped by the same radius the renderer uses.
  for (const id of ids) {
    const r = nodeRadius(backlinksOf.get(id) || 0);
    pos[id].x = Math.min(width - r - 6, Math.max(r + 6, pos[id].x));
    pos[id].y = Math.min(height - r - 6, Math.max(r + 6, pos[id].y));
  }

  return pos;
}

// ------------------------------------------------------------------ routing
//
// `app.js`'s router cuts a view's own tail at the first segment equal to
// `"task"` -- that is `MODAL` there, its way of telling a task-modal
// boundary apart from a view's own segments (`splitTail`, `tailOf`). A note
// id is a wiki path, and nothing on disk stops an area or a filename from
// being spelled `task` (`operations/task.md`), so writing an id's segments
// to the tail verbatim can hand the router a false boundary. A `note/`
// prefix does not save it either: `indexOf(MODAL)` matches *any* position in
// the array, not just the first, so a later segment named `task` still cuts
// there. What survives is keeping the literal string `"task"` out of the
// tail entirely -- a segment that *is* `task` is written as `task~` (a
// trailing `~` is unreserved and passes `encodeURIComponent`/
// `decodeURIComponent` unchanged, the same trick `writeHash` in scopes.js
// uses for a scope literally named `all`) and reversed on read. The one case
// this does not cover is a wiki that also has a path segment already spelled
// `task~`, which is the same kind of edge case `all`/`%61ll` already accepts.
const RESERVED = "task";

/// Pure: a note or gap id to the tail segments that name it. Exported
/// alongside `readNoteTail` so a round-trip -- including one through the
/// reserved word -- is testable without booting `app.js` (importing it runs
/// `boot()` against `document`, which Node has none of).
export function noteTail(id) {
  if (!id) return [];
  return id.split("/").map((seg) => (seg === RESERVED ? `${RESERVED}~` : seg));
}

/// The inverse of `noteTail`. `null` for an empty tail, meaning no selection.
export function readNoteTail(segments) {
  if (!segments || !segments.length) return null;
  return segments.map((seg) => (seg === `${RESERVED}~` ? RESERVED : seg)).join("/");
}

/// The id of the note or gap currently shown in the inspector. Module state
/// rather than `state.knowledge` because it is a fact about what this view is
/// showing, not about the last fetch -- the same reason `workflows.js` keeps
/// `current`/`selectedNode` of its own.
let selected = null;

/// The graph's own SVG, rebuilt only when the payload it was built from
/// changes (compared by reference, set in `loadKnowledge`) -- never on a
/// plain selection change or a scope-rail rerender. Rebuilding replaces every
/// node element, which would steal focus from a keyboard user mid-selection;
/// `scopes.js`'s rail keeps the same kind of guard (`drawn`) for the same
/// reason.
let graphBuiltFor = null;

export function knowledgeTail() {
  return noteTail(selected);
}

export function readKnowledgeTail(segments) {
  selected = readNoteTail(segments);
  // The data may not have loaded yet (a fresh deep link arrives before
  // `onShow`'s fetch resolves) -- `renderKnowledge` degrades gracefully with
  // `state.knowledge` still null, and `loadKnowledge`'s own
  // `settleSelection` corrects an id the fetch turns out not to know about.
  renderKnowledge();
}

/// The note with the most backlinks, or `null` when there are no notes to
/// choose from ("or none" -- the issue leaves the choice to this change). A
/// tie falls to whichever note the reduce visits first; since the payload
/// sorts notes by id, that is the alphabetically first of the tied ids,
/// which is at least a stable and repeatable answer.
function defaultNote(data) {
  const notes = data.notes || [];
  if (!notes.length) return null;
  return notes.reduce((best, n) => ((n.backlinks || []).length > (best.backlinks || []).length ? n : best)).id;
}

/// Called after every fetch: drops a selection the new answer does not know
/// about, fills in the default when there is none, and corrects the hash to
/// match -- a replace, never a push, the same "settle then correct" shape
/// `workflows.js`'s `readWorkflowTail` uses for the same reason (a boot
/// deep-link should not cost a Back press to get past).
function settleSelection() {
  const data = state.knowledgeError ? null : state.knowledge;
  if (!data || !data.present) return;
  const known = (id) => (data.notes || []).some((n) => n.id === id) || (data.gaps || []).some((g) => g.target === id);
  if (selected && !known(selected)) selected = null;
  if (selected === null) selected = defaultNote(data);
  writeHash(true);
}

function selectNote(id) {
  if (id === selected) return;
  selected = id;
  updateGraphSelection();
  const data = state.knowledgeError ? null : state.knowledge;
  if (data && data.present) renderInspector(data);
  writeHash();
}

// -------------------------------------------------------------------- graph

function truncateLabel(s, max = 20) {
  const str = String(s ?? "");
  return str.length > max ? `${str.slice(0, max - 1)}…` : str;
}

function graphEdges(notes) {
  const edges = [];
  for (const n of notes) {
    for (const t of n.links || []) edges.push({ from: n.id, to: t, dashed: false });
    for (const t of n.gaps || []) edges.push({ from: n.id, to: t, dashed: true });
  }
  return edges;
}

function buildGraphSvg(data) {
  const svg = $("knowledge-graph");
  if (!svg) return;
  const notes = data.notes || [];
  const gaps = data.gaps || [];
  const width = 640;
  const height = 420;
  const pos = layoutGraph(notes, gaps, { width, height });
  const backlinksOf = new Map(notes.map((n) => [n.id, (n.backlinks || []).length]));
  const titleOf = new Map(notes.map((n) => [n.id, n.title]));
  const edges = graphEdges(notes);

  const edgeSvg = edges
    .map((e) => {
      const a = pos[e.from];
      const b = pos[e.to];
      if (!a || !b) return "";
      return `<line class="know-edge${e.dashed ? " gap" : ""}" data-from="${esc(e.from)}" data-to="${esc(e.to)}"
        x1="${a.x.toFixed(1)}" y1="${a.y.toFixed(1)}" x2="${b.x.toFixed(1)}" y2="${b.y.toFixed(1)}"></line>`;
    })
    .join("");

  const ids = Object.keys(pos);
  const nodeSvg = ids
    .map((id) => {
      const isGap = !backlinksOf.has(id);
      const r = nodeRadius(backlinksOf.get(id) || 0);
      const p = pos[id];
      const label = isGap ? id : titleOf.get(id) || id;
      const ariaLabel = isGap ? `Gap: ${label}, linked but never written` : `Note: ${label}`;
      return `<g class="know-node${isGap ? " gap" : ""}" data-id="${esc(id)}" tabindex="0" role="button"
          aria-label="${esc(ariaLabel)}" transform="translate(${p.x.toFixed(1)},${p.y.toFixed(1)})">
        <circle r="${r}"></circle>
      </g>`;
    })
    .join("");

  // Labels are their own layer, drawn last so a dashed edge crossing a node
  // or a neighbouring node's circle can never paint over a label -- see the
  // QA finding on this. Each also carries a halo (`.know-label` in
  // app.css) for the same reason where a label still crosses an edge that
  // runs behind it. `nodeRadius` plus a 12px clearance keeps a label below
  // its own node's circle regardless of layer order.
  const labelSvg = ids
    .map((id) => {
      const isGap = !backlinksOf.has(id);
      const r = nodeRadius(backlinksOf.get(id) || 0);
      const p = pos[id];
      const label = isGap ? id : titleOf.get(id) || id;
      return `<text class="know-label${isGap ? " gap" : ""}" data-id="${esc(id)}"
          x="${p.x.toFixed(1)}" y="${(p.y + r + 12).toFixed(1)}" text-anchor="middle">${esc(truncateLabel(label))}</text>`;
    })
    .join("");

  svg.innerHTML = `<g class="know-edges">${edgeSvg}</g><g class="know-nodes">${nodeSvg}</g><g class="know-labels">${labelSvg}</g>`;
  for (const g of svg.querySelectorAll(".know-node")) {
    g.onclick = () => selectNote(g.dataset.id);
    g.onkeydown = (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault(); // " " scrolls the page otherwise -- the node is not a link
        selectNote(g.dataset.id);
      }
    };
  }
  updateGraphSelection();
}

function updateGraphSelection() {
  const svg = $("knowledge-graph");
  if (!svg) return;
  for (const g of svg.querySelectorAll(".know-node")) {
    g.classList.toggle("selected", g.dataset.id === selected);
  }
  for (const t of svg.querySelectorAll(".know-label")) {
    t.classList.toggle("selected", t.dataset.id === selected);
  }
  for (const line of svg.querySelectorAll(".know-edge")) {
    line.classList.toggle("selected", line.dataset.from === selected || line.dataset.to === selected);
  }
}

// --------------------------------------------------------------- inspector

function goto(id) {
  return `<button type="button" class="linklike" data-goto="${esc(id)}">`;
}

function pointsAtList(note, data) {
  const notesById = new Map(data.notes.map((n) => [n.id, n]));
  const resolved = (note.links || []).map((id) => {
    const n = notesById.get(id);
    return `<li>${goto(id)}${esc(n ? n.title : id)}</button></li>`;
  });
  const gaps = (note.gaps || []).map(
    (id) => `<li>${goto(id)}${esc(id)}</button> <span class="tag warn">unwritten</span></li>`,
  );
  const items = resolved.concat(gaps).join("");
  return items || `<li class="sub">Nothing.</li>`;
}

function pointedAtByList(ids, data) {
  const notesById = new Map(data.notes.map((n) => [n.id, n]));
  const items = (ids || [])
    .map((id) => {
      const n = notesById.get(id);
      return `<li>${goto(id)}${esc(n ? n.title : id)}</button></li>`;
    })
    .join("");
  return items || `<li class="sub">Nothing.</li>`;
}

function renderInspector(data) {
  const el = $("knowledge-inspector");
  if (!el) return;
  if (!selected) {
    el.innerHTML = `<p class="sub">Select a note or a gap in the graph, or a row in the tables below, to inspect it.</p>`;
    return;
  }
  const note = (data.notes || []).find((n) => n.id === selected);
  if (note) {
    el.innerHTML = `
      <h3>${esc(note.title)}</h3>
      <dl class="know-facts">
        <div><dt>Id</dt><dd><code>${esc(note.id)}</code></dd></div>
        <div><dt>Area</dt><dd>${esc(note.area || "—")}</dd></div>
        <div><dt>Status</dt><dd>${esc(note.status || "—")}</dd></div>
        <div><dt>Updated</dt><dd>${esc(note.updated || "—")}</dd></div>
        <div><dt>Sources</dt><dd>${esc(note.sources)}</dd></div>
      </dl>
      <h4>Points at</h4>
      <ul class="know-linklist">${pointsAtList(note, data)}</ul>
      <h4>Pointed at by</h4>
      <ul class="know-linklist">${pointedAtByList(note.backlinks, data)}</ul>`;
  } else {
    const gap = (data.gaps || []).find((g) => g.target === selected);
    el.innerHTML = `
      <h3><code>${esc(selected)}</code></h3>
      <p class="sub">Linked but never written.</p>
      <h4>Pointed at by</h4>
      <ul class="know-linklist">${pointedAtByList(gap ? gap.from : [], data)}</ul>`;
  }
  for (const b of el.querySelectorAll("[data-goto]")) {
    b.onclick = () => selectNote(b.dataset.goto);
  }
}

// -------------------------------------------------------- gaps, findings, notes

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
  unsourced: "Unsourced notes",
  secret_source: "Sources under data/secrets/",
  missing_source: "Sources not found",
  incomplete_frontmatter: "Incomplete frontmatter",
  orphan: "Orphans — nothing links here",
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

function noteRow(n) {
  return `<tr>
    <td><div class="title">${goto(n.id)}${esc(n.title)}</button></div></td>
    <td><code>${esc(n.id)}</code></td>
    <td>${esc(n.area || "—")}</td>
    <td>${esc(n.status || "—")}</td>
    <td>${(n.backlinks || []).length}</td>
    <td>${(n.links || []).length}</td>
    <td>${esc(n.sources)}</td>
  </tr>`;
}

// ------------------------------------------------------------------- render

export function renderKnowledge() {
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
    if (data && data.present === false) empty.textContent = `No wiki at ${data.root}`;
  }

  const shell = $("knowledge-shell");
  if (shell) shell.hidden = !present;

  const count = $("knowledge-count");
  if (count) {
    if (present) {
      const notes = data.notes || [];
      const links = notes.reduce((n, x) => n + (x.links || []).length, 0);
      const gaps = (data.gaps || []).length;
      count.textContent = `${notes.length} note${notes.length === 1 ? "" : "s"} · ${links} link${links === 1 ? "" : "s"} · ${gaps} gap${gaps === 1 ? "" : "s"}`;
    } else {
      count.textContent = "";
    }
  }

  if (present) {
    if (graphBuiltFor !== data) {
      buildGraphSvg(data);
      graphBuiltFor = data;
    } else {
      updateGraphSelection();
    }
    renderInspector(data);
  } else {
    const svg = $("knowledge-graph");
    if (svg) svg.innerHTML = "";
    graphBuiltFor = null;
    const inspector = $("knowledge-inspector");
    if (inspector) inspector.innerHTML = "";
  }

  const gaps = present ? data.gaps || [] : [];
  const gapsBody = $("knowledge-gaps");
  if (gapsBody) {
    gapsBody.innerHTML = gaps.map(gapRow).join("");
    for (const b of gapsBody.querySelectorAll("[data-goto]")) b.onclick = () => selectNote(b.dataset.goto);
  }
  const noGaps = $("knowledge-no-gaps");
  if (noGaps) noGaps.hidden = !present || gaps.length !== 0;

  renderFindings(present ? data.findings : []);
  const noFindings = $("knowledge-no-findings");
  if (noFindings) noFindings.hidden = !present || (data.findings || []).length !== 0;

  const notes = present ? data.notes || [] : [];
  const notesBody = $("knowledge-notes");
  if (notesBody) {
    notesBody.innerHTML = notes.map(noteRow).join("");
    for (const b of notesBody.querySelectorAll("[data-goto]")) b.onclick = () => selectNote(b.dataset.goto);
  }
  const noNotes = $("knowledge-no-notes");
  if (noNotes) noNotes.hidden = !present || notes.length !== 0;

  const pagesNote = $("knowledge-pages-note");
  if (pagesNote) {
    const pages = present ? data.pages || [] : [];
    pagesNote.textContent = present
      ? `${pages.length} page${pages.length === 1 ? "" : "s"} without frontmatter ${pages.length === 1 ? "is" : "are"} not in the graph.`
      : "";
    // Otherwise this renders as an empty bordered box whenever the fetch
    // fails, or on any other render with no text -- `hidden` whenever there
    // is nothing to show, not just whenever the fetch specifically failed.
    pagesNote.hidden = !pagesNote.textContent;
  }
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
