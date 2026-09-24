//! Pure graph mechanics for the L5 Knowledge tab: layout, the local-graph
//! neighbourhood, the toolbar's filter, and the hash-tail id encoding.
//! Nothing here touches `document` -- `knowledge.js` is the only module that
//! draws anything, so this one is safe to import in a Node test with no DOM
//! at all (see `ui/tests/improvement.test.js`).
//!
//! `layoutGraph` is the same deterministic force layout v1 shipped for
//! notes and gaps, generalised to whichever nodes and edges the caller
//! hands it -- a page, a tag, a document or a gap all lay out the same way.
//! Seeded from a hash of the node ids, never `Math.random`, so the same
//! vault draws the same picture on every load. A 600-node graph has to
//! settle in under 2s in a Node test; the repulsion pass buckets nodes into
//! a coarse grid rather than comparing every pair, which is what keeps a
//! large vault's layout from becoming an O(n^2) wall.

// ------------------------------------------------------------ deterministic layout
//
// A tiny seeded PRNG (mulberry32) rather than `Math.random`, seeded from a
// hash of the node ids themselves -- never from insertion order, never from
// wall-clock time -- so the same vault produces the same picture on every
// load, in every browser, regardless of what order the payload happened to
// list things in.

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

/// A page node's on-screen radius: bigger for a page more other pages point
/// at, so the graph's own geometry says which pages the vault leans on
/// without reading the table beside it. Tag, document and gap nodes are
/// drawn at a fixed size of their own in `knowledge.js` -- this is a page's
/// formula alone, kept under its old name from v1.
const BASE_RADIUS = 10;
const RADIUS_PER_BACKLINK = 2;
const MAX_RADIUS = 26;
export function nodeRadius(backlinks) {
  const n = Number(backlinks) || 0;
  return Math.min(MAX_RADIUS, BASE_RADIUS + RADIUS_PER_BACKLINK * n);
}

/// The side of a grid cell used to bucket repulsion below -- comfortably
/// larger than a spread-out node's radius, so two nodes that could plausibly
/// repel each other are never split across cells that never compare.
const GRID_CELL = 70;

/// `nodes` is `[{id, r}]` -- `r` the radius `knowledge.js` will actually draw
/// that node at, which is what the repulsion and the final clamp both need;
/// `edges` is `[[a, b]]`, either id resolving to nothing simply drawing no
/// line. `{width, height}` is the viewBox the caller intends to draw into.
/// Returns `{id: {x, y}}` for every node, pure and deterministic: the same
/// nodes and edges, in any order, produce the same coordinates every time,
/// and every coordinate lands within the box.
///
/// The force loop itself -- a spring per edge, a pull to centre, a fixed
/// iteration count rather than a convergence check (a convergence check is
/// itself a source of nondeterminism across engines) -- is v1's, unchanged.
/// Only the repulsion pass is different: past a few hundred nodes, comparing
/// every pair is the one part of this that would not finish in time, so
/// nodes are bucketed into a grid each iteration and a node only repels
/// whatever shares its cell or a neighbouring one.
export function layoutGraph(nodes, edges, { width = 640, height = 420 } = {}) {
  const nodeList = nodes || [];
  const edgeList = edges || [];
  if (!nodeList.length) return {};

  const radiusOf = new Map(nodeList.map((n) => [n.id, Number(n.r) || BASE_RADIUS]));
  // Sorted explicitly, and not trusted from the caller's order -- seeding and
  // iterating both depend on it, and a fixture built in a different order
  // must still land on the same picture.
  const ids = Array.from(new Set(nodeList.map((n) => n.id))).sort();

  const rand = mulberry32(hashSeed(ids));
  const pos = {};
  for (const id of ids) {
    pos[id] = {
      x: width / 2 + (rand() - 0.5) * width * 0.7,
      y: height / 2 + (rand() - 0.5) * height * 0.7,
    };
  }

  const knownEdges = edgeList.filter(([a, b]) => pos[a] && pos[b]);

  const cx = width / 2;
  const cy = height / 2;
  const ITERATIONS = 240;
  for (let iter = 0; iter < ITERATIONS; iter++) {
    const disp = new Map(ids.map((id) => [id, { x: 0, y: 0 }]));

    // Repulsion, bucketed: a coarse grid over the current positions, then
    // only compare a node against the same cell and its eight neighbours --
    // everything farther away is close enough to zero force at this scale
    // that skipping it never changes where the graph settles.
    const grid = new Map();
    const cellOf = (p) => `${Math.floor(p.x / GRID_CELL)}:${Math.floor(p.y / GRID_CELL)}`;
    for (const id of ids) {
      const key = cellOf(pos[id]);
      if (!grid.has(key)) grid.set(key, []);
      grid.get(key).push(id);
    }
    for (const id of ids) {
      const [cxk, cyk] = cellOf(pos[id]).split(":").map(Number);
      for (let gx = cxk - 1; gx <= cxk + 1; gx++) {
        for (let gy = cyk - 1; gy <= cyk + 1; gy++) {
          const bucket = grid.get(`${gx}:${gy}`);
          if (!bucket) continue;
          for (const other of bucket) {
            if (other <= id) continue; // each unordered pair once
            let dx = pos[id].x - pos[other].x;
            let dy = pos[id].y - pos[other].y;
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
            disp.get(id).x += fx;
            disp.get(id).y += fy;
            disp.get(other).x -= fx;
            disp.get(other).y -= fy;
          }
        }
      }
    }

    // Springs, one per edge -- pulls a linked node and its target together.
    for (const [a, b] of knownEdges) {
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
  // viewBox -- scale-and-translate the settled bounding box up to fill it,
  // leaving room on every side for a label. This runs before the per-node
  // radius clamp below, so whatever this step produces still gets pulled
  // back inside the frame.
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
    const r = radiusOf.get(id) || BASE_RADIUS;
    pos[id].x = Math.min(width - r - 6, Math.max(r + 6, pos[id].x));
    pos[id].y = Math.min(height - r - 6, Math.max(r + 6, pos[id].y));
  }

  return pos;
}

// -------------------------------------------------------------- neighbourhood

/// Every node id reachable from `centerId` in at most `depth` hops, `centerId`
/// itself included -- pure breadth-first search over `edges` (`[[a, b]]`,
/// walked as undirected: a document a page references is one hop from that
/// page, and the page is one hop back). `depth` below 1 still returns just
/// the centre; a `centerId` no edge names still returns `{centerId}` rather
/// than throwing.
export function neighborhood(centerId, edges, depth) {
  const seen = new Set([centerId]);
  if (!centerId || depth < 1) return seen;
  const adjacency = new Map();
  const link = (a, b) => {
    if (!adjacency.has(a)) adjacency.set(a, new Set());
    adjacency.get(a).add(b);
  };
  for (const [a, b] of edges || []) {
    link(a, b);
    link(b, a);
  }
  let frontier = [centerId];
  for (let hop = 0; hop < depth; hop++) {
    const next = [];
    for (const id of frontier) {
      for (const neighbor of adjacency.get(id) || []) {
        if (!seen.has(neighbor)) {
          seen.add(neighbor);
          next.push(neighbor);
        }
      }
    }
    frontier = next;
    if (!frontier.length) break;
  }
  return seen;
}

// ------------------------------------------------------------------ filtering
//
// The toolbar's tag/document/gap toggles decide which *kinds* of node are
// eligible to draw at all, applied by `knowledge.js`'s own `buildNodes`
// before any of the below ever runs. The Orphans toggle is different: it
// is not about a node's kind at all, but about whether it ends up with a
// neighbour once the other toggles have already been applied -- see
// `connectedNodeIds`.

/// The ids, among `nodeIds`, with at least one edge to another id in
/// `nodeIds` -- i.e. "has a neighbour in the graph as currently drawn."
/// This is what the Orphans toggle actually means (Obsidian's own
/// reading): a node whose only edge points at something the *other*
/// toggles have hidden is exactly as isolated on screen as one with no
/// edge recorded anywhere, so the check has to run after those toggles,
/// against the node list they already narrowed -- never against the
/// `orphan` finding, which answers a different question (nothing *points
/// at* this page) and stays exactly as it is for the findings table.
export function connectedNodeIds(nodeIds, edges) {
  const ids = new Set(nodeIds);
  const connected = new Set();
  for (const [a, b] of edges || []) {
    if (ids.has(a) && ids.has(b)) {
      connected.add(a);
      connected.add(b);
    }
  }
  return connected;
}

/// Whether `query` (already lowercased and trimmed by the caller) matches a
/// node well enough to stay lit rather than dimmed: its label, its id, or --
/// for a tag -- its bare name without the leading `#`. An empty query
/// matches everything, which is what "no search typed" should do.
export function matchesSearch(node, query) {
  if (!query) return true;
  const hay = [node.id, node.label, node.kind === "tag" ? node.id.replace(/^tag:/, "") : ""]
    .join(" ")
    .toLowerCase();
  return hay.includes(query);
}

/// Whether `node` should be dimmed right now. While `query` is non-empty,
/// dimming is decided purely by whether the node matches it -- the
/// neighbour-highlight below does not also apply on top, so a match that
/// happens not to sit next to whatever is focused still stays lit. With no
/// query, dimming follows hover/keyboard focus alone: a node is dimmed only
/// once something is focused, and only when it is neither the focus itself
/// nor one of its immediate neighbours. Deliberately never driven by a
/// *selection* -- `knowledge.js` marks that with its own `.selected` class
/// instead of dimming the rest of the graph, since a default first-load
/// selection (or a page found by search) should not make most of a small
/// vault look greyed out.
///
/// `hits`, when given, is the set of page ids a provider search returned
/// (the "Search" form above the graph, not the filter box in its toolbar):
/// while it is showing, everything else is dimmed, whatever is hovered, so
/// the answer stays readable on the graph until it is cleared. The filter
/// box still wins over it -- typing there is the more recent question.
export function isNodeDimmed(node, query, focus, neighbours, hits) {
  if (query) return !matchesSearch(node, query);
  if (hits) return !hits.has(node.id);
  if (!focus) return false;
  return node.id !== focus && !(neighbours && neighbours.has(node.id));
}

/// The URL a provider search is asked at: the words as typed, since the
/// provider reads `#tag` and plain words alike, and an explicit limit so the
/// page shows the same number of hits whatever the daemon's default becomes.
export function knowledgeSearchUrl(text, limit = 20) {
  return `/api/knowledge/search?q=${encodeURIComponent(text.trim())}&limit=${limit}`;
}

/// Six colour tokens (the same `--ly1`..`--ly6` cycle the site plan's
/// directory tiles use), assigned to `areas` by each area's position in the
/// *sorted list of distinct areas actually present* -- not a hash of its
/// name mod 6, which collides often enough in a real vault that two or
/// three areas land on the same colour. Sorting first makes the assignment
/// depend only on which areas exist, never on what order pages happened to
/// list them in, so the same vault still draws the same picture on every
/// load. Past six distinct areas the cycle repeats; a blank/`null` area
/// (a page at the vault root) never gets an entry.
const AREA_TOKENS = ["--ly1", "--ly2", "--ly3", "--ly4", "--ly5", "--ly6"];
export function areaColorTokens(areas) {
  const order = [...new Set(areas)].filter(Boolean).sort();
  const map = new Map();
  order.forEach((area, i) => map.set(area, AREA_TOKENS[i % AREA_TOKENS.length]));
  return map;
}

/// How far the graph has to be zoomed in before every label shows, not just
/// the focused/selected node and its neighbours -- past this, a large vault
/// is not yet a smear of text. Below it, a *small* graph (at most
/// `LABEL_ALWAYS_SHOW_NODE_COUNT` nodes) still shows every label regardless
/// of zoom: the threshold exists to keep a big vault legible, not to hide
/// the handful of labels an ordinary small one would otherwise draw with
/// none at all.
const LABEL_ZOOM_THRESHOLD = 1.6;
const LABEL_ALWAYS_SHOW_NODE_COUNT = 60;
export function shouldShowAllLabels(nodeCount, scale) {
  return nodeCount <= LABEL_ALWAYS_SHOW_NODE_COUNT || scale >= LABEL_ZOOM_THRESHOLD;
}

// -------------------------------------------------------------- pan & zoom
//
// `knowledge.js` draws into a fixed 640x420 viewBox and applies its own
// `translate(tx,ty) scale(s)` on top of that for panning and zooming. SVG's
// transform-list order scales a point first and translates it second, so
// `tx`/`ty` already live in the *output* (viewBox) space rather than
// needing a further division by `s` anywhere below -- that division is
// exactly what used to make panning drift and zooming creep toward the
// origin the more zoomed in the graph already was.

/// The viewBox-units-per-screen-pixel ratio for an SVG using
/// `preserveAspectRatio="xMidYMid meet"`: the smaller of the two axis
/// scale-up factors is the one that "meets" the box edge to edge and
/// applies to *both* axes alike (the other one only looks larger because it
/// letterboxes), so `viewBox.width / rect.width` alone is only right when
/// the rendered box happens to share the viewBox's own aspect ratio.
export function unitsPerPixel(viewBox, rect) {
  if (!rect || !rect.width || !rect.height) return 1;
  return Math.max(viewBox.width / rect.width, viewBox.height / rect.height);
}

/// A screen point (`clientX`/`clientY`, page coordinates) as a point in the
/// fixed viewBox space `zoomAt`'s `anchor` lives in -- the wheel handler's
/// own use for this. `unitsPerPixel` alone is not enough here: whichever
/// axis has slack under `xMidYMid meet` is letterboxed, so the viewBox's
/// content starts inset from `rect.left`/`rect.top` by half that slack, not
/// flush with it. Skipping the inset (as if the content always started at
/// the rect's own corner) anchors a wheel-zoom off wherever the two axes'
/// unscaled ratios happen to differ -- exactly the case `unitsPerPixel`
/// itself exists to handle correctly for the *scale*, but not, on its own,
/// for *where the origin sits*.
export function viewBoxPoint(viewBox, rect, clientX, clientY) {
  const ppu = unitsPerPixel(viewBox, rect);
  const offX = (rect.width - viewBox.width / ppu) / 2;
  const offY = (rect.height - viewBox.height / ppu) / 2;
  return { x: (clientX - rect.left - offX) * ppu, y: (clientY - rect.top - offY) * ppu };
}

/// `view` (`{scale, tx, ty}`) after zooming by `factor` around `anchor`
/// (`{x, y}`, in the same fixed viewBox space `tx`/`ty` live in) -- the
/// local point currently drawn under `anchor` stays under it once the new
/// scale is applied, which is what keeps `+`/`-`/wheel from drifting the
/// picture toward the top-left corner. `min`/`max` clamp the resulting
/// scale; the caller passes the viewport's centre for the toolbar buttons
/// and the cursor's own position for the wheel.
export function zoomAt(view, factor, anchor, { min = 0.5, max = 4 } = {}) {
  const nextScale = Math.min(max, Math.max(min, view.scale * factor));
  const localX = (anchor.x - view.tx) / view.scale;
  const localY = (anchor.y - view.ty) / view.scale;
  return { scale: nextScale, tx: anchor.x - localX * nextScale, ty: anchor.y - localY * nextScale };
}

/// `{tx, ty}` after dragging the pointer by `(dxScreen, dyScreen)` screen
/// pixels, given the view's `tx`/`ty` before the drag and `ppu` (from
/// `unitsPerPixel`) to convert screen pixels to viewBox units. Deliberately
/// not divided by `view.scale` anywhere: `tx`/`ty` are already in
/// scaled-output space, so a second division would make the picture pan
/// faster than the pointer the more zoomed in it already was.
export function panBy(view, dxScreen, dyScreen, ppu) {
  return { tx: view.tx + dxScreen * ppu, ty: view.ty + dyScreen * ppu };
}

// ----------------------------------------------------------------- hash tail
//
// `app.js`'s router cuts a view's own tail at the first segment equal to
// `"task"` -- that is `MODAL` there, its way of telling a task-modal
// boundary apart from a view's own segments (`splitTail`, `tailOf`). A
// selection id is a vault path (a page id), a `tag:<name>`, or a document's
// own path, and nothing stops any of those from containing a segment
// spelled `task` (`operations/task.md`, `tag:task`), so writing it to the
// tail verbatim can hand the router a false boundary. What survives is
// keeping the literal string `"task"` out of the tail entirely -- a segment
// that *is* `task` is written as `task~` (a trailing `~` is unreserved and
// passes `encodeURIComponent`/`decodeURIComponent` unchanged, the same trick
// `writeHash` in scopes.js uses for a scope literally named `all`) and
// reversed on read.
const RESERVED = "task";

/// The toolbar's four toggles, whether local-graph mode is on, and its depth,
/// packed into one fixed-shape tail segment so all of it survives a reload:
/// `f-<tags>-<documents>-<orphans>-<gaps>-<local>-<depth>`, each flag `0`/`1`
/// and depth `1`-`3`. Pure functions of a plain `{tags, documents, orphans,
/// gaps, local, depth}` object, kept apart from `knowledge.js`'s own toolbar
/// state so the encoding can be pinned in a test with no view behind it.
const FLAGS_RE = /^f-([01])-([01])-([01])-([01])-([01])-([123])$/;

export function encodeToolbarFlags(t) {
  return `f-${t.tags ? 1 : 0}-${t.documents ? 1 : 0}-${t.orphans ? 1 : 0}-${t.gaps ? 1 : 0}-${t.local ? 1 : 0}-${t.depth}`;
}

/// `null` when `segment` does not match the shape at all -- an old link, or
/// none written yet -- so the caller can fall back to its defaults rather
/// than misreading an unrelated segment as flags.
export function decodeToolbarFlags(segment) {
  const m = FLAGS_RE.exec(segment || "");
  if (!m) return null;
  return {
    tags: m[1] === "1",
    documents: m[2] === "1",
    orphans: m[3] === "1",
    gaps: m[4] === "1",
    local: m[5] === "1",
    depth: Number(m[6]),
  };
}

/// Pure: a page id, `tag:<name>`, document path or gap target to the tail
/// segments that name it.
export function nodeTail(id) {
  if (!id) return [];
  return id.split("/").map((seg) => (seg === RESERVED ? `${RESERVED}~` : seg));
}

/// The inverse of `nodeTail`. `null` for an empty tail, meaning no selection.
export function readNodeTail(segments) {
  if (!segments || !segments.length) return null;
  return segments.map((seg) => (seg === `${RESERVED}~` ? RESERVED : seg)).join("/");
}
