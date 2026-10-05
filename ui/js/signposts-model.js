//! Read-only scenario observations, independent of process attention.
//! Never convert a signpost into a task/action or count it as waiting work.
export function signpostObservations(fact, href = "") {
  return (Array.isArray(fact?.triggered) ? fact.triggered : [])
    .filter((row) => typeof row?.scenario === "string" && typeof row.metric === "string" && typeof row.reason === "string")
    .map((row) => ({
      kind: "signpost", observation: true, tone: "muted",
      title: row.scenario, label: row.metric, reason: row.reason,
      href, age: null, actions: [],
    }));
}
