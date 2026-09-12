//! Pure graph helpers shared by the workflow canvas and its tests.

export function topologicalSummary(workflow) {
  const incoming = new Map(workflow.nodes.map(node => [node.id, []]));
  for (const edge of workflow.edges) if (incoming.has(edge.to)) incoming.get(edge.to).push(edge.from);
  const remaining = new Set(workflow.nodes.map(node => node.id)), ordered = [];
  while (remaining.size) {
    const ready = [...remaining].filter(id => incoming.get(id).every(parent => !remaining.has(parent)));
    if (!ready.length) return { error: `Cycle involving ${[...remaining].join(", ")}`, nodes: [] };
    for (const id of ready) { remaining.delete(id); ordered.push({ id, after: incoming.get(id) }); }
  }
  return { error: null, nodes: ordered };
}
