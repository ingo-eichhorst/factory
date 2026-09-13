//! Pure graph helpers shared by the workflow canvas and its tests. Nothing
//! here touches the DOM or the HTTP API -- it is the shape of a workflow's
//! nodes and edges (and, for layout, their canvas positions), and nothing
//! else. `workflow-model.js` builds the domain rules (validation, editing,
//! events) on top of this; this file only ever answers questions a graph
//! could answer about itself.

const NODE_W = 184;
const NODE_H = 84;
const GAP = 18;

/// Whether a pointer travelled far enough between `start` and `end` to count
/// as a drag rather than a click. Chrome dispatches a zero-distance
/// `pointermove` right after `setPointerCapture` on an ordinary click (R1),
/// so treating any `pointermove` at all as a drag misreads a click as one --
/// distance is the only signal that actually tells them apart. `threshold`
/// is in the same units as `start`/`end` (screen px, when called from the
/// canvas); real movement must exceed it, so a zero threshold still requires
/// a nonzero distance.
export function isDrag(start, end, threshold = 3) {
  return Math.hypot(end.x - start.x, end.y - start.y) > threshold;
}

/// A node's title, or its id when it somehow has none -- used everywhere a
/// message needs to name a node the way the canvas does rather than by its
/// opaque id alone.
export function nodeTitle(nodes, id) {
  return nodes.find(node => node.id === id)?.task?.title || id;
}

/// The topological order, doubling as the accessible textual summary (see
/// issue #45's "ordered textual node/edge summary" requirement). A cycle is
/// reported by naming every node still stuck in it, by title as well as id --
/// "node-80e3b710-..." means nothing on a canvas; the title on the card does.
export function topologicalSummary(workflow) {
  const incoming = new Map(workflow.nodes.map(node => [node.id, []]));
  for (const edge of workflow.edges) {
    if (incoming.has(edge.to)) incoming.get(edge.to).push(edge.from);
  }
  const remaining = new Set(workflow.nodes.map(node => node.id));
  const ordered = [];
  while (remaining.size) {
    const ready = [...remaining].filter(id => incoming.get(id).every(parent => !remaining.has(parent)));
    if (!ready.length) {
      const named = [...remaining]
        .map(id => `${JSON.stringify(nodeTitle(workflow.nodes, id))} (${id})`)
        .join(", ");
      return { error: `workflow contains a cycle involving ${named}`, nodes: [] };
    }
    for (const id of ready) {
      remaining.delete(id);
      ordered.push({ id, after: incoming.get(id) });
    }
  }
  return { error: null, nodes: ordered };
}

/// Root (start) nodes: every node with no incoming edge. The canvas marks
/// these distinctly (U6); the summary lists them as "after start".
export function rootIds(nodes, edges) {
  const withIncoming = new Set(edges.map(edge => edge.to));
  return nodes.filter(node => !withIncoming.has(node.id)).map(node => node.id);
}

function adjacency(edges) {
  const map = new Map();
  for (const edge of edges) {
    if (!map.has(edge.from)) map.set(edge.from, []);
    map.get(edge.from).push(edge.to);
  }
  return map;
}

/// True when `target` can be reached from `start` by following edges
/// forward. Adding an edge `from -> to` closes a cycle exactly when `from`
/// is already reachable from `to` -- that is the one check `connectionError`
/// needs, so this is the only graph search either of them does.
export function reachable(edges, start, target) {
  const map = adjacency(edges);
  const seen = new Set([start]);
  const stack = [start];
  while (stack.length) {
    const current = stack.pop();
    if (current === target) return true;
    for (const next of map.get(current) || []) {
      if (!seen.has(next)) {
        seen.add(next);
        stack.push(next);
      }
    }
  }
  return false;
}

/// Why linking `fromId -> toId` is refused, or `null` when it is fine --
/// named by node title (U6: "the error names the node titles").
export function connectionError(nodes, edges, fromId, toId) {
  if (fromId === toId) {
    return `"${nodeTitle(nodes, fromId)}" cannot link to itself`;
  }
  if (edges.some(edge => edge.from === fromId && edge.to === toId)) {
    return `"${nodeTitle(nodes, fromId)}" already links to "${nodeTitle(nodes, toId)}"`;
  }
  if (reachable(edges, toId, fromId)) {
    return `linking "${nodeTitle(nodes, fromId)}" to "${nodeTitle(nodes, toId)}" would create a cycle`;
  }
  return null;
}

/// Deleting a node takes its incident edges with it -- a dangling edge is
/// never a state the model produces on its own.
export function removeNode(nodes, edges, nodeId) {
  return {
    nodes: nodes.filter(node => node.id !== nodeId),
    edges: edges.filter(edge => edge.from !== nodeId && edge.to !== nodeId),
  };
}

export function removeEdge(edges, edgeId) {
  return edges.filter(edge => edge.id !== edgeId);
}

/// A position near `(x, y)` that does not overlap any existing node's card,
/// walking diagonally until it clears every one of them -- used for both
/// "Add task" and "Duplicate" so neither ever stacks a new card exactly on
/// top of another (U11).
export function freePosition(nodes, x, y) {
  let px = x;
  let py = y;
  let guard = 0;
  const overlaps = () =>
    nodes.some(
      node =>
        Math.abs(node.position.x - px) < NODE_W + GAP &&
        Math.abs(node.position.y - py) < NODE_H + GAP,
    );
  while (overlaps() && guard++ < 500) {
    px += 36;
    py += 28;
  }
  return { x: px, y: py };
}

/// The 8 statuses a node run can carry, in the order the issue lists them.
/// `app.css`'s `.workflow-node.wf-s-*` rules give each a distinct look; this
/// is just which class name a status maps to, defaulting an unknown one to
/// `unstarted` rather than drawing nothing.
export const NODE_STATUSES = [
  "unstarted",
  "pending",
  "dispatching",
  "running",
  "blocked",
  "done",
  "failed",
  "cancelled",
  "skipped",
];

export function nodeStatusClass(status) {
  return `wf-s-${NODE_STATUSES.includes(status) ? status : "unstarted"}`;
}

/// An edge reflects progress: once its source is done, it is drawn in the
/// run colour instead of the idle one (U5/U6).
export function edgeStatusClass(fromStatus) {
  return fromStatus === "done" ? "wf-edge-done" : "";
}

/// A `{x, y, zoom}` view that fits every node's card inside a `width` x
/// `height` viewport, with a margin. Used both the moment a workflow opens
/// and by the toolbar's "Fit" button (U11); an empty graph gets the same
/// default the canvas has always opened at.
export function fitView(nodes, width, height) {
  if (!nodes.length) return { x: 40, y: 40, zoom: 1 };
  const margin = 60;
  const minX = Math.min(...nodes.map(node => node.position.x));
  const minY = Math.min(...nodes.map(node => node.position.y));
  const maxX = Math.max(...nodes.map(node => node.position.x + NODE_W));
  const maxY = Math.max(...nodes.map(node => node.position.y + NODE_H));
  const spanX = Math.max(1, maxX - minX);
  const spanY = Math.max(1, maxY - minY);
  const zoom = Math.max(
    0.3,
    Math.min(1.5, Math.min((width - margin * 2) / spanX, (height - margin * 2) / spanY)),
  );
  return { zoom, x: margin - minX * zoom, y: margin - minY * zoom };
}
